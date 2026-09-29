//! Dictionary store: the SQLite table that owns every entry.
//!
//! One table in the History database (migration in `managers/history.rs`).
//! Every producer writes through [`DictionaryManager`]. The consumer (the
//! transcription pipeline) never touches the database: it reads
//! [`active_entries`], an in-memory matcher that this module rebuilds after
//! each write. That keeps the hot path at one `Arc` clone.
//!
//! Rules (design doc section 5):
//! - `wrong` and `right` are stored as written. `wrong_key`/`right_key` are
//!   lowercase copies used only for uniqueness.
//! - A pair that exists again raises `seen_count`. It never makes a second row.
//! - A new learned pair activates automatically only when the conservative
//!   classifier supplies a context guard and no active rule already owns the
//!   same wrong text. Other learned pairs start as `proposed`.
//! - Re-observing a pair only raises `seen_count`; it never changes a prior
//!   user decision or promotes a suggestion.
//! - One active replacement per `wrong`: activating a pair for the same wrong
//!   text demotes the old one to `proposed` in the same transaction.
//! - Only `state = 'active'` and `enabled = 1` entries apply.

use crate::dictionary::{CaseMode, DictionaryEntry};
use crate::dictionary_learning::{classify_pairs, LearningCandidate};
use crate::dictionary_matcher::{DictionaryMatcher, DictionaryRule};
use anyhow::{anyhow, Result};
use log::{info, warn};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use tauri::{AppHandle, Emitter};

/// Emitted after every change. Windows reload their copy of the list.
pub const ENTRIES_CHANGED_EVENT: &str = "dictionary-entries-changed";

/// One row of the `dictionary` table, as the frontend sees it.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Type)]
pub struct DictionaryRow {
    pub id: i64,
    pub wrong: String,
    pub right: String,
    pub case_mode: CaseMode,
    /// "manual", "history", or "capture".
    pub source: String,
    /// "active", "proposed", or "rejected".
    pub state: String,
    pub enabled: bool,
    /// True only while this row is governed by automatic-learning safeguards.
    pub auto_learned: bool,
    /// Local context guard for an automatically active row.
    pub context_words: Vec<String>,
    pub seen_count: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Result of learning from one edit.
#[derive(Serialize, Deserialize, Debug, Clone, Type)]
pub struct LearnReport {
    /// Rows this edit created.
    pub added: Vec<DictionaryRow>,
    /// Rows that already existed; their `seen_count` went up.
    pub known: Vec<DictionaryRow>,
}

// ---------------------------------------------------------------------------
// Hot-path snapshot
// ---------------------------------------------------------------------------

static ACTIVE: OnceLock<RwLock<Arc<DictionaryMatcher>>> = OnceLock::new();

fn active_cell() -> &'static RwLock<Arc<DictionaryMatcher>> {
    ACTIVE.get_or_init(|| RwLock::new(Arc::new(DictionaryMatcher::default())))
}

/// Matcher that applies now. One `Arc` clone; safe to call on the paste path.
pub fn active_entries() -> Arc<DictionaryMatcher> {
    match active_cell().read() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

fn set_active_entries(matcher: DictionaryMatcher) {
    let next = Arc::new(matcher);
    match active_cell().write() {
        Ok(mut guard) => *guard = next,
        Err(poisoned) => *poisoned.into_inner() = next,
    }
}

// ---------------------------------------------------------------------------
// Manager
// ---------------------------------------------------------------------------

fn key(s: &str) -> String {
    s.trim().to_lowercase()
}

fn case_mode_str(mode: CaseMode) -> &'static str {
    match mode {
        CaseMode::Smart => "smart",
        CaseMode::Exact => "exact",
    }
}

fn parse_case_mode(s: &str) -> CaseMode {
    if s == "exact" {
        CaseMode::Exact
    } else {
        CaseMode::Smart
    }
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

pub struct DictionaryManager {
    app_handle: AppHandle,
    db_path: PathBuf,
    /// Keep the committed table and the hot-path snapshot in the same order
    /// when capture and UI commands write at the same time.
    mutation_lock: Mutex<()>,
}

impl DictionaryManager {
    /// Opens the store. `HistoryManager::new` must have run first: it owns the
    /// migrations. Imports entries left in settings by earlier test builds,
    /// then builds the hot-path snapshot.
    pub fn new(app_handle: &AppHandle) -> Result<Self> {
        let app_data_dir = crate::portable::app_data_dir(app_handle)?;
        let manager = Self {
            app_handle: app_handle.clone(),
            db_path: app_data_dir.join("history.db"),
            mutation_lock: Mutex::new(()),
        };
        if let Err(err) = manager.import_from_settings() {
            warn!("dictionary: import from settings failed: {err}");
        }
        manager.refresh_cache()?;
        Ok(manager)
    }

    fn conn(&self) -> Result<Connection> {
        Ok(Connection::open(&self.db_path)?)
    }

    fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DictionaryRow> {
        let case_mode: String = row.get("case_mode")?;
        Ok(DictionaryRow {
            id: row.get("id")?,
            wrong: row.get("wrong")?,
            right: row.get("right")?,
            case_mode: parse_case_mode(&case_mode),
            source: row.get("source")?,
            state: row.get("state")?,
            enabled: row.get::<_, i64>("enabled")? != 0,
            auto_learned: row.get::<_, i64>("auto_learned")? != 0,
            context_words: serde_json::from_str(&row.get::<_, String>("context_words")?)
                .unwrap_or_default(),
            seen_count: row.get("seen_count")?,
            created_at: row.get("created_at")?,
            updated_at: row.get("updated_at")?,
        })
    }

    const SELECT: &'static str = "SELECT id, wrong, right, case_mode, source, state, enabled, \
         auto_learned, context_words, seen_count, created_at, updated_at FROM dictionary";

    /// Every row, newest change first.
    pub fn list(&self) -> Result<Vec<DictionaryRow>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(&format!(
            "{} ORDER BY updated_at DESC, id DESC",
            Self::SELECT
        ))?;
        let rows = stmt
            .query_map([], Self::map_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    fn get_by_keys(
        conn: &Connection,
        wrong_key: &str,
        right_key: &str,
    ) -> Result<Option<DictionaryRow>> {
        Ok(conn
            .query_row(
                &format!(
                    "{} WHERE wrong_key = ?1 AND right_key = ?2 AND app_id = ''",
                    Self::SELECT
                ),
                params![wrong_key, right_key],
                Self::map_row,
            )
            .optional()?)
    }

    fn get_by_id(conn: &Connection, id: i64) -> Result<Option<DictionaryRow>> {
        Ok(conn
            .query_row(
                &format!("{} WHERE id = ?1", Self::SELECT),
                params![id],
                Self::map_row,
            )
            .optional()?)
    }

    /// One active replacement per wrong text. Demote any other active row
    /// for `wrong_key` before a new one becomes active.
    fn demote_other_active(conn: &Connection, wrong_key: &str, keep_right_key: &str) -> Result<()> {
        if wrong_key.is_empty() {
            return Ok(());
        }
        conn.execute(
            "UPDATE dictionary SET state = 'proposed', updated_at = ?3
             WHERE wrong_key = ?1 AND app_id = '' AND state = 'active' AND right_key != ?2",
            params![wrong_key, keep_right_key, now()],
        )?;
        Ok(())
    }

    /// Insert or bump one explicit pair inside an open transaction. This is
    /// used by manual adds and the one-time settings import.
    fn upsert(
        conn: &Connection,
        entry: &DictionaryEntry,
        source: &str,
        activate: bool,
    ) -> Result<(DictionaryRow, bool)> {
        let wrong = entry.wrong.trim();
        let right = entry.right.trim();
        if right.is_empty() {
            return Err(anyhow!("right text is empty"));
        }
        let (wk, rk) = (key(wrong), key(right));
        let ts = now();
        let is_new = Self::get_by_keys(conn, &wk, &rk)?.is_none();
        if activate {
            Self::demote_other_active(conn, &wk, &rk)?;
        }
        conn.execute(
            "INSERT INTO dictionary
               (wrong, right, match_mode, case_mode, source, state, enabled, seen_count,
                applied_count, app_id, wrong_key, right_key, created_at, updated_at,
                auto_learned, context_words)
             VALUES (?1, ?2, 'word', ?3, ?4, ?5, 1, 1, 0, '', ?6, ?7, ?8, ?8,
                     0, '[]')
             ON CONFLICT(wrong_key, right_key, app_id) DO UPDATE SET
               seen_count = seen_count + 1,
               state = CASE WHEN ?9 THEN 'active' ELSE state END,
               enabled = CASE WHEN ?9 THEN 1 ELSE enabled END,
               auto_learned = CASE WHEN ?9 THEN 0 ELSE auto_learned END,
               context_words = CASE WHEN ?9 THEN '[]' ELSE context_words END,
               updated_at = excluded.updated_at",
            params![
                wrong,
                right,
                case_mode_str(entry.case_mode),
                source,
                if activate { "active" } else { "proposed" },
                wk,
                rk,
                ts,
                activate,
            ],
        )?;
        let row = Self::get_by_keys(conn, &wk, &rk)?
            .ok_or_else(|| anyhow!("row missing after upsert"))?;
        Ok((row, is_new))
    }

    fn has_active_wrong(conn: &Connection, wrong_key: &str) -> Result<bool> {
        Ok(conn.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM dictionary
                 WHERE wrong_key = ?1 AND app_id = '' AND state = 'active'
             )",
            params![wrong_key],
            |row| row.get(0),
        )?)
    }

    /// Store classifier output. Automatic activation is decided only when a
    /// pair is first inserted. Existing proposed, rejected, disabled, or
    /// explicitly approved rows retain their complete lifecycle state.
    fn learn_candidates(
        conn: &Connection,
        candidates: &[LearningCandidate],
        source: &str,
    ) -> Result<LearnReport> {
        let mut report = LearnReport {
            added: Vec::new(),
            known: Vec::new(),
        };
        for candidate in candidates {
            let entry = &candidate.entry;
            let wrong = entry.wrong.trim();
            let right = entry.right.trim();
            if right.is_empty() {
                continue;
            }
            let (wk, rk) = (key(wrong), key(right));
            if let Some(existing) = Self::get_by_keys(conn, &wk, &rk)? {
                conn.execute(
                    "UPDATE dictionary
                     SET seen_count = seen_count + 1, updated_at = ?2
                     WHERE id = ?1",
                    params![existing.id, now()],
                )?;
                report.known.push(
                    Self::get_by_id(conn, existing.id)?
                        .ok_or_else(|| anyhow!("row missing after observation"))?,
                );
                continue;
            }

            let activate =
                !candidate.context_words.is_empty() && !Self::has_active_wrong(conn, &wk)?;
            // Suggestions deliberately carry no guard. A later repeat cannot
            // promote them, and explicit confirmation makes them user-owned.
            let context_words = if activate {
                serde_json::to_string(&candidate.context_words)?
            } else {
                "[]".to_string()
            };
            let ts = now();
            conn.execute(
                "INSERT INTO dictionary
                   (wrong, right, match_mode, case_mode, source, state, enabled,
                    seen_count, applied_count, app_id, wrong_key, right_key,
                    created_at, updated_at, auto_learned, context_words)
                 VALUES (?1, ?2, 'word', ?3, ?4, ?5, 1, 1, 0, '', ?6, ?7,
                         ?8, ?8, ?10, ?9)",
                params![
                    wrong,
                    right,
                    case_mode_str(entry.case_mode),
                    source,
                    if activate { "active" } else { "proposed" },
                    wk,
                    rk,
                    ts,
                    context_words,
                    activate,
                ],
            )?;
            report.added.push(
                Self::get_by_keys(conn, &key(wrong), &key(right))?
                    .ok_or_else(|| anyhow!("row missing after learning"))?,
            );
        }
        Ok(report)
    }

    fn explicit_trusted_terms(conn: &Connection) -> Result<HashSet<String>> {
        let mut stmt = conn.prepare(
            "SELECT right FROM dictionary
             WHERE state = 'active' AND enabled = 1 AND auto_learned = 0",
        )?;
        let mut terms = HashSet::new();
        for right in stmt.query_map([], |row| row.get::<_, String>(0))? {
            let term = key(&right?);
            if !term.is_empty() {
                terms.insert(term);
            }
        }
        Ok(terms)
    }

    /// Classify an observed edit and store its candidates. This work is never
    /// called from the paste path. The lock keeps the trusted-vocabulary read,
    /// classification decision, and database write in one mutation order.
    pub fn learn_from_edit(
        &self,
        original: &str,
        corrected: &str,
        source: &str,
    ) -> Result<LearnReport> {
        let _guard = self
            .mutation_lock
            .lock()
            .map_err(|_| anyhow!("dictionary mutation lock poisoned"))?;
        let mut conn = self.conn()?;
        let mut trusted_terms: HashSet<String> = crate::settings::get_settings(&self.app_handle)
            .custom_words
            .into_iter()
            .filter_map(|word| {
                let word = key(&word);
                (!word.is_empty()).then_some(word)
            })
            .collect();
        trusted_terms.extend(Self::explicit_trusted_terms(&conn)?);

        let mut candidates = classify_pairs(original, corrected, &trusted_terms);
        if source == "capture" {
            candidates.truncate(3);
        }
        if candidates.is_empty() {
            return Ok(LearnReport {
                added: Vec::new(),
                known: Vec::new(),
            });
        }
        let tx = conn.transaction()?;
        let report = Self::learn_candidates(&tx, &candidates, source)?;
        tx.commit()?;
        self.notify();
        crate::correction_notices::enqueue(&self.app_handle, report.added.clone());
        Ok(report)
    }

    /// Manual add from the Dictionary screen. Exact case: it is what the user
    /// typed. A pair that already exists is an error the screen reports.
    pub fn add_manual(&self, wrong: &str, right: &str) -> Result<DictionaryRow> {
        let (wrong, right) = (wrong.trim(), right.trim());
        if wrong.is_empty() || right.is_empty() {
            return Err(anyhow!("both texts are required"));
        }
        let _guard = self
            .mutation_lock
            .lock()
            .map_err(|_| anyhow!("dictionary mutation lock poisoned"))?;
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        if Self::get_by_keys(&tx, &key(wrong), &key(right))?.is_some() {
            return Err(anyhow!("duplicate"));
        }
        let entry = DictionaryEntry {
            wrong: wrong.to_string(),
            right: right.to_string(),
            case_mode: CaseMode::Exact,
            source: "manual".to_string(),
        };
        let (row, _) = Self::upsert(&tx, &entry, "manual", true)?;
        tx.commit()?;
        self.notify();
        Ok(row)
    }

    /// Edit a row in place and set whether it is enabled. This does not confirm
    /// a proposal or revive a rejected row; those state transitions use the
    /// explicit confirmation command.
    pub fn update(
        &self,
        id: i64,
        wrong: &str,
        right: &str,
        case_mode: CaseMode,
        enabled: bool,
    ) -> Result<DictionaryRow> {
        let (wrong, right) = (wrong.trim(), right.trim());
        if wrong.is_empty() || right.is_empty() {
            return Err(anyhow!("both texts are required"));
        }
        let _guard = self
            .mutation_lock
            .lock()
            .map_err(|_| anyhow!("dictionary mutation lock poisoned"))?;
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        let row = Self::update_row(&tx, id, wrong, right, case_mode, enabled)?;
        tx.commit()?;
        self.notify();
        crate::correction_notices::remove_entry(&self.app_handle, id);
        Ok(row)
    }

    fn update_row(
        conn: &Connection,
        id: i64,
        wrong: &str,
        right: &str,
        case_mode: CaseMode,
        enabled: bool,
    ) -> Result<DictionaryRow> {
        let (wk, rk) = (key(wrong), key(right));
        let existing = Self::get_by_id(conn, id)?.ok_or_else(|| anyhow!("entry {id} not found"))?;
        if let Some(other) = Self::get_by_keys(conn, &wk, &rk)? {
            if other.id != id {
                return Err(anyhow!("duplicate"));
            }
        }
        if existing.state == "active" && enabled {
            Self::demote_other_active(conn, &wk, &rk)?;
        }
        let text_changed = wrong != existing.wrong || right != existing.right;
        let clear_automatic_guard = existing.state == "active" && text_changed;
        let changed = conn.execute(
            "UPDATE dictionary SET wrong = ?2, right = ?3, case_mode = ?4,
                wrong_key = ?5, right_key = ?6,
                enabled = ?7, updated_at = ?8,
                auto_learned = CASE WHEN ?9 THEN 0 ELSE auto_learned END,
                context_words = CASE WHEN ?9 THEN '[]' ELSE context_words END
             WHERE id = ?1",
            params![
                id,
                wrong,
                right,
                case_mode_str(case_mode),
                wk,
                rk,
                enabled,
                now(),
                clear_automatic_guard,
            ],
        )?;
        debug_assert_eq!(changed, 1);
        Self::get_by_keys(conn, &wk, &rk)?.ok_or_else(|| anyhow!("row missing"))
    }

    /// Accept a learned proposal. This is the only learned-entry path that can
    /// replace an already approved correction for the same wrong text.
    pub fn confirm_dictionary_entry(&self, id: i64) -> Result<DictionaryRow> {
        let _guard = self
            .mutation_lock
            .lock()
            .map_err(|_| anyhow!("dictionary mutation lock poisoned"))?;
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        let row = Self::confirm(&tx, id)?;
        tx.commit()?;
        self.notify();
        crate::correction_notices::remove_entry(&self.app_handle, id);
        Ok(row)
    }

    fn confirm(conn: &Connection, id: i64) -> Result<DictionaryRow> {
        let row = Self::get_by_id(conn, id)?.ok_or_else(|| anyhow!("entry {id} not found"))?;
        let (wk, rk) = (key(&row.wrong), key(&row.right));
        Self::demote_other_active(conn, &wk, &rk)?;
        conn.execute(
            "UPDATE dictionary
             SET state = 'active', enabled = 1, auto_learned = 0,
                 context_words = '[]', updated_at = ?2
             WHERE id = ?1",
            params![id, now()],
        )?;
        Self::get_by_id(conn, id)?.ok_or_else(|| anyhow!("row missing after confirmation"))
    }

    /// Dismiss a learned proposal. Future observations still increase its
    /// count, but learning never changes it out of the rejected state.
    pub fn reject_dictionary_entry(&self, id: i64) -> Result<DictionaryRow> {
        let _guard = self
            .mutation_lock
            .lock()
            .map_err(|_| anyhow!("dictionary mutation lock poisoned"))?;
        let conn = self.conn()?;
        let row = Self::reject(&conn, id)?;
        self.notify();
        crate::correction_notices::remove_entry(&self.app_handle, id);
        Ok(row)
    }

    fn reject(conn: &Connection, id: i64) -> Result<DictionaryRow> {
        let changed = conn.execute(
            "UPDATE dictionary SET state = 'rejected', updated_at = ?2 WHERE id = ?1",
            params![id, now()],
        )?;
        if changed == 0 {
            return Err(anyhow!("entry {id} not found"));
        }
        Self::get_by_id(conn, id)?.ok_or_else(|| anyhow!("row missing after rejection"))
    }

    pub fn delete(&self, id: i64) -> Result<bool> {
        let _guard = self
            .mutation_lock
            .lock()
            .map_err(|_| anyhow!("dictionary mutation lock poisoned"))?;
        let conn = self.conn()?;
        let changed = conn.execute("DELETE FROM dictionary WHERE id = ?1", params![id])?;
        if changed > 0 {
            self.notify();
            crate::correction_notices::remove_entry(&self.app_handle, id);
        }
        Ok(changed > 0)
    }

    /// Rebuild the hot-path snapshot from the table.
    pub fn refresh_cache(&self) -> Result<()> {
        let conn = self.conn()?;
        // Build the automaton before taking the global write lock. Readers see
        // either the complete old matcher or the complete new matcher.
        let matcher = DictionaryMatcher::new(Self::load_active_rules(&conn)?)?;
        set_active_entries(matcher);
        Ok(())
    }

    /// Legacy entry-only loader retained for focused lifecycle tests.
    #[cfg(test)]
    fn load_active_entries(conn: &Connection) -> Result<Vec<DictionaryEntry>> {
        let mut stmt = conn.prepare(
            "SELECT wrong, right, case_mode, source FROM dictionary
             WHERE state = 'active' AND enabled = 1 ORDER BY id",
        )?;
        let entries = stmt
            .query_map([], |row| {
                let case_mode: String = row.get(2)?;
                Ok(DictionaryEntry {
                    wrong: row.get(0)?,
                    right: row.get(1)?,
                    case_mode: parse_case_mode(&case_mode),
                    source: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(entries)
    }

    fn load_active_rules(conn: &Connection) -> Result<Vec<DictionaryRule>> {
        let mut stmt = conn.prepare(
            "SELECT wrong, right, case_mode, source, auto_learned, context_words
             FROM dictionary
             WHERE state = 'active' AND enabled = 1 ORDER BY id",
        )?;
        let rows = stmt.query_map([], |row| {
            let case_mode: String = row.get(2)?;
            Ok((
                DictionaryEntry {
                    wrong: row.get(0)?,
                    right: row.get(1)?,
                    case_mode: parse_case_mode(&case_mode),
                    source: row.get(3)?,
                },
                row.get::<_, i64>(4)? != 0,
                row.get::<_, String>(5)?,
            ))
        })?;

        let mut rules = Vec::new();
        for row in rows {
            let (entry, auto_learned, encoded_context) = row?;
            let context_words = if auto_learned {
                // Automatic rules fail closed if their guard is absent or the
                // persisted JSON is corrupt. Explicit rules need no guard.
                let Ok(words) = serde_json::from_str::<Vec<String>>(&encoded_context) else {
                    warn!("dictionary: skipping automatic row with invalid context guard");
                    continue;
                };
                if words.is_empty() {
                    warn!("dictionary: skipping automatic row with empty context guard");
                    continue;
                }
                words
            } else {
                Vec::new()
            };
            rules.push(DictionaryRule {
                entry,
                context_words,
            });
        }
        Ok(rules)
    }

    fn notify(&self) {
        if let Err(err) = self.refresh_cache() {
            warn!("dictionary: cache refresh failed: {err}");
            // The database mutation already committed. Keeping the previous
            // matcher could continue applying a rule the user just rejected,
            // disabled, edited, or deleted, so fail closed until a later
            // successful rebuild.
            set_active_entries(DictionaryMatcher::default());
        }
        let _ = self.app_handle.emit(ENTRIES_CHANGED_EVENT, ());
    }

    /// One-time move of entries that earlier test builds kept in settings.
    /// After the import the settings list is emptied; the table owns them.
    fn import_from_settings(&self) -> Result<()> {
        let mut settings = crate::settings::get_settings(&self.app_handle);
        if settings.dictionary_entries.is_empty() {
            return Ok(());
        }
        let count = settings.dictionary_entries.len();
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        for entry in &settings.dictionary_entries {
            let source = if entry.source.is_empty() {
                "manual"
            } else {
                entry.source.as_str()
            };
            let activate = source == "manual";
            if let Err(err) = Self::upsert(&tx, entry, source, activate) {
                warn!("dictionary: skipped one settings entry on import: {err}");
            }
        }
        tx.commit()?;
        settings.dictionary_entries.clear();
        crate::settings::write_settings(&self.app_handle, settings);
        info!("dictionary: imported {count} entries from settings into SQLite");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE dictionary (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                wrong TEXT NOT NULL DEFAULT '', right TEXT NOT NULL,
                match_mode TEXT NOT NULL DEFAULT 'word', case_mode TEXT NOT NULL DEFAULT 'smart',
                source TEXT NOT NULL, state TEXT NOT NULL DEFAULT 'active',
                enabled INTEGER NOT NULL DEFAULT 1, seen_count INTEGER NOT NULL DEFAULT 1,
                applied_count INTEGER NOT NULL DEFAULT 0, app_id TEXT NOT NULL DEFAULT '',
                wrong_key TEXT NOT NULL, right_key TEXT NOT NULL,
                created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
                auto_learned INTEGER NOT NULL DEFAULT 0,
                context_words TEXT NOT NULL DEFAULT '[]');
             CREATE UNIQUE INDEX dictionary_pair ON dictionary (wrong_key, right_key, app_id);
             CREATE UNIQUE INDEX dictionary_active_wrong ON dictionary (wrong_key, app_id)
                WHERE state = 'active' AND wrong_key != '';",
        )
        .unwrap();
        conn
    }

    fn entry(wrong: &str, right: &str) -> DictionaryEntry {
        DictionaryEntry {
            wrong: wrong.into(),
            right: right.into(),
            case_mode: CaseMode::Smart,
            source: "capture".into(),
        }
    }

    fn candidate(wrong: &str, right: &str, context_words: &[&str]) -> LearningCandidate {
        LearningCandidate {
            entry: entry(wrong, right),
            context_words: context_words
                .iter()
                .map(|word| (*word).to_string())
                .collect(),
        }
    }

    #[test]
    fn upsert_bumps_seen_count_instead_of_duplicating() {
        let conn = fresh();
        let (a, new_a) =
            DictionaryManager::upsert(&conn, &entry("Bededa", "Pereira"), "capture", false)
                .unwrap();
        let (b, new_b) =
            DictionaryManager::upsert(&conn, &entry("bededa", "Pereira"), "history", false)
                .unwrap();
        assert!(new_a);
        assert!(!new_b);
        assert_eq!(a.id, b.id);
        assert_eq!(b.seen_count, 2);
        assert_eq!(b.state, "proposed");
        assert_eq!(b.source, "capture", "the first producer's source stays");
    }

    #[test]
    fn learned_conflict_does_not_demote_active_rule_until_confirmed() {
        let conn = fresh();
        let (old, _) =
            DictionaryManager::upsert(&conn, &entry("Maine", "main"), "manual", true).unwrap();
        let (new, _) =
            DictionaryManager::upsert(&conn, &entry("Maine", "Mane"), "history", false).unwrap();
        let old_now = conn
            .query_row(
                &format!("{} WHERE id = ?1", DictionaryManager::SELECT),
                params![old.id],
                DictionaryManager::map_row,
            )
            .unwrap();
        assert_eq!(old_now.state, "active");
        assert_eq!(new.state, "proposed");

        let confirmed = DictionaryManager::confirm(&conn, new.id).unwrap();
        let old_after = DictionaryManager::get_by_id(&conn, old.id)
            .unwrap()
            .unwrap();
        assert_eq!(confirmed.state, "active");
        assert!(confirmed.enabled);
        assert_eq!(old_after.state, "proposed");
    }

    #[test]
    fn rejected_rows_stay_rejected_on_repeat() {
        let conn = fresh();
        let (active, _) =
            DictionaryManager::upsert(&conn, &entry("we are", "were"), "manual", true).unwrap();
        let (row, _) =
            DictionaryManager::upsert(&conn, &entry("we are", "we're"), "capture", false).unwrap();
        DictionaryManager::reject(&conn, row.id).unwrap();
        let (again, is_new) =
            DictionaryManager::upsert(&conn, &entry("we are", "we're"), "capture", false).unwrap();
        assert!(!is_new);
        assert_eq!(again.state, "rejected");
        assert_eq!(again.seen_count, 2);
        assert_eq!(
            DictionaryManager::get_by_id(&conn, active.id)
                .unwrap()
                .unwrap()
                .state,
            "active"
        );
    }

    #[test]
    fn repeat_of_active_pair_stays_active_without_becoming_new() {
        let conn = fresh();
        let (active, _) =
            DictionaryManager::upsert(&conn, &entry("Handy", "handy"), "manual", true).unwrap();
        let (again, is_new) =
            DictionaryManager::upsert(&conn, &entry("Handy", "handy"), "capture", false).unwrap();
        assert!(!is_new);
        assert_eq!(again.id, active.id);
        assert_eq!(again.state, "active");
        assert_eq!(again.seen_count, 2);
    }

    #[test]
    fn manual_rows_are_active_immediately() {
        let conn = fresh();
        let (row, is_new) =
            DictionaryManager::upsert(&conn, &entry("handy", "Handy"), "manual", true).unwrap();
        assert!(is_new);
        assert_eq!(row.state, "active");
        assert!(row.enabled);
    }

    #[test]
    fn active_snapshot_excludes_proposed_rejected_and_disabled_rows() {
        let conn = fresh();
        let (active, _) =
            DictionaryManager::upsert(&conn, &entry("one", "ONE"), "manual", true).unwrap();
        let (proposed, _) =
            DictionaryManager::upsert(&conn, &entry("two", "TWO"), "capture", false).unwrap();
        let (rejected, _) =
            DictionaryManager::upsert(&conn, &entry("three", "THREE"), "history", false).unwrap();
        DictionaryManager::reject(&conn, rejected.id).unwrap();
        let (disabled, _) =
            DictionaryManager::upsert(&conn, &entry("four", "FOUR"), "manual", true).unwrap();
        conn.execute(
            "UPDATE dictionary SET enabled = 0 WHERE id = ?1",
            params![disabled.id],
        )
        .unwrap();

        let snapshot = DictionaryManager::load_active_entries(&conn).unwrap();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].wrong, active.wrong);
        assert!(snapshot.iter().all(|entry| entry.wrong != proposed.wrong));
    }

    #[test]
    fn strong_new_candidate_activates_with_context_guard() {
        let conn = fresh();
        let report = DictionaryManager::learn_candidates(
            &conn,
            &[candidate("Catapult", "Katapult", &["releases", "team"])],
            "history",
        )
        .unwrap();

        assert_eq!(report.added.len(), 1);
        let row = &report.added[0];
        assert_eq!(row.state, "active");
        assert!(row.auto_learned);
        assert_eq!(row.context_words, ["releases", "team"]);
        let rules = DictionaryManager::load_active_rules(&conn).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].context_words, row.context_words);
    }

    #[test]
    fn weak_candidate_is_a_guardless_suggestion() {
        let conn = fresh();
        let report = DictionaryManager::learn_candidates(
            &conn,
            &[candidate("Catapult", "Katapult", &[])],
            "capture",
        )
        .unwrap();

        let row = &report.added[0];
        assert_eq!(row.state, "proposed");
        assert!(!row.auto_learned);
        assert!(row.context_words.is_empty());
        assert!(DictionaryManager::load_active_rules(&conn)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn automatic_active_rows_with_missing_or_corrupt_guards_fail_closed() {
        let conn = fresh();
        for (wrong, context_words) in [("missing", "[]"), ("corrupt", "not-json")] {
            conn.execute(
                "INSERT INTO dictionary
                   (wrong, right, source, state, enabled, wrong_key, right_key,
                    created_at, updated_at, auto_learned, context_words)
                 VALUES (?1, ?2, 'history', 'active', 1, ?1, ?2, 1, 1, 1, ?3)",
                params![wrong, format!("{wrong}-right"), context_words],
            )
            .unwrap();
        }
        assert!(DictionaryManager::load_active_rules(&conn)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn existing_active_wrong_blocks_automatic_activation_even_when_disabled() {
        let conn = fresh();
        let (explicit, _) =
            DictionaryManager::upsert(&conn, &entry("Maine", "main"), "manual", true).unwrap();
        conn.execute(
            "UPDATE dictionary SET enabled = 0 WHERE id = ?1",
            params![explicit.id],
        )
        .unwrap();

        let report = DictionaryManager::learn_candidates(
            &conn,
            &[candidate("Maine", "Mane", &["branch"])],
            "history",
        )
        .unwrap();
        assert_eq!(report.added[0].state, "proposed");
        assert!(!report.added[0].auto_learned);
        assert!(report.added[0].context_words.is_empty());
        assert_eq!(
            DictionaryManager::get_by_id(&conn, explicit.id)
                .unwrap()
                .unwrap()
                .state,
            "active"
        );
    }

    #[test]
    fn repeats_never_promote_suggestions_or_rejected_rows() {
        let conn = fresh();
        let weak = candidate("Catapult", "Katapult", &[]);
        let proposed = DictionaryManager::learn_candidates(&conn, &[weak.clone()], "history")
            .unwrap()
            .added
            .remove(0);
        let strong = candidate("Catapult", "Katapult", &["releases"]);
        let repeated = DictionaryManager::learn_candidates(&conn, &[strong], "capture")
            .unwrap()
            .known
            .remove(0);
        assert_eq!(repeated.id, proposed.id);
        assert_eq!(repeated.state, "proposed");
        assert!(!repeated.auto_learned);
        assert!(repeated.context_words.is_empty());

        DictionaryManager::reject(&conn, proposed.id).unwrap();
        let ignored = DictionaryManager::learn_candidates(&conn, &[weak], "history")
            .unwrap()
            .known
            .remove(0);
        assert_eq!(ignored.state, "rejected");
        assert_eq!(ignored.seen_count, 3);
    }

    #[test]
    fn automatic_rows_do_not_seed_trusted_vocabulary() {
        let conn = fresh();
        DictionaryManager::learn_candidates(
            &conn,
            &[candidate("catapult", "Katapult", &["team"])],
            "history",
        )
        .unwrap();
        DictionaryManager::upsert(&conn, &entry("github", "GitHub"), "manual", true).unwrap();

        let trusted = DictionaryManager::explicit_trusted_terms(&conn).unwrap();
        assert!(trusted.contains("github"));
        assert!(!trusted.contains("katapult"));
    }

    #[test]
    fn explicit_confirmation_and_active_text_edit_clear_automatic_guard() {
        let conn = fresh();
        let confirmed_auto = DictionaryManager::learn_candidates(
            &conn,
            &[candidate("Catapult", "Katapult", &["team"])],
            "history",
        )
        .unwrap()
        .added
        .remove(0);

        let confirmed = DictionaryManager::confirm(&conn, confirmed_auto.id).unwrap();
        assert!(!confirmed.auto_learned);
        assert!(confirmed.context_words.is_empty());

        let editable_auto = DictionaryManager::learn_candidates(
            &conn,
            &[candidate("Bededa", "Pereira", &["flight"])],
            "capture",
        )
        .unwrap()
        .added
        .remove(0);
        let toggled = DictionaryManager::update_row(
            &conn,
            editable_auto.id,
            &editable_auto.wrong,
            &editable_auto.right,
            editable_auto.case_mode,
            false,
        )
        .unwrap();
        assert!(
            toggled.auto_learned,
            "a toggle preserves automatic metadata"
        );
        assert_eq!(toggled.context_words, ["flight"]);

        let edited = DictionaryManager::update_row(
            &conn,
            editable_auto.id,
            &editable_auto.wrong,
            "Pereira Airport",
            editable_auto.case_mode,
            true,
        )
        .unwrap();
        assert!(!edited.auto_learned);
        assert!(edited.context_words.is_empty());
    }

    #[test]
    fn explicit_upsert_clears_existing_automatic_metadata() {
        let conn = fresh();
        let automatic = DictionaryManager::learn_candidates(
            &conn,
            &[candidate("Catapult", "Katapult", &["team"])],
            "history",
        )
        .unwrap()
        .added
        .remove(0);
        assert!(automatic.auto_learned);

        let (explicit, is_new) =
            DictionaryManager::upsert(&conn, &entry("Catapult", "Katapult"), "manual", true)
                .unwrap();
        assert!(!is_new);
        assert_eq!(explicit.state, "active");
        assert!(!explicit.auto_learned);
        assert!(explicit.context_words.is_empty());
    }

    #[test]
    fn trusted_edit_applies_only_in_context_and_rejection_is_sticky() {
        let conn = fresh();
        DictionaryManager::upsert(&conn, &entry("Katapault", "Katapult"), "manual", true).unwrap();
        let trusted = DictionaryManager::explicit_trusted_terms(&conn).unwrap();
        assert!(trusted.contains("katapult"));

        let original = "Catapult manages our releases daily";
        let corrected = "Katapult manages our releases daily";
        let candidates = classify_pairs(original, corrected, &trusted);
        assert_eq!(candidates.len(), 1);
        assert!(!candidates[0].context_words.is_empty());
        let learned = DictionaryManager::learn_candidates(&conn, &candidates, "history")
            .unwrap()
            .added
            .remove(0);
        assert_eq!(learned.state, "active");
        assert!(learned.auto_learned);

        let matcher =
            DictionaryMatcher::new(DictionaryManager::load_active_rules(&conn).unwrap()).unwrap();
        assert_eq!(matcher.apply(original), corrected);
        assert_eq!(
            matcher.apply("A catapult launches rocks"),
            "A catapult launches rocks"
        );

        DictionaryManager::reject(&conn, learned.id).unwrap();
        let rejected_matcher =
            DictionaryMatcher::new(DictionaryManager::load_active_rules(&conn).unwrap()).unwrap();
        assert_eq!(rejected_matcher.apply(original), original);

        let repeated = DictionaryManager::learn_candidates(&conn, &candidates, "capture")
            .unwrap()
            .known
            .remove(0);
        assert_eq!(repeated.state, "rejected");
        assert_eq!(repeated.seen_count, 2);
        let repeated_matcher =
            DictionaryMatcher::new(DictionaryManager::load_active_rules(&conn).unwrap()).unwrap();
        assert_eq!(repeated_matcher.apply(original), original);
    }
}
