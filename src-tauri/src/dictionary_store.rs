//! Dictionary store: the SQLite table that owns every entry.
//!
//! One table in the History database (migration in `managers/history.rs`).
//! Every producer writes through [`DictionaryManager`]. The consumer (the
//! transcription pipeline) never touches the database: it reads
//! [`active_entries`], an in-memory snapshot that this module rebuilds after
//! each write. That keeps the hot path at one `Arc` clone.
//!
//! Rules (design doc section 5):
//! - `wrong` and `right` are stored as written. `wrong_key`/`right_key` are
//!   lowercase copies used only for uniqueness.
//! - A pair that exists again raises `seen_count`. It never makes a second row.
//! - One active replacement per `wrong`: a new active pair for the same wrong
//!   text demotes the old one to `proposed`. The last correction wins.
//! - Only `state = 'active'` and `enabled = 1` entries apply.

use crate::dictionary::{CaseMode, DictionaryEntry};
use anyhow::{anyhow, Result};
use log::{info, warn};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};
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

static ACTIVE: OnceLock<RwLock<Arc<Vec<DictionaryEntry>>>> = OnceLock::new();

fn active_cell() -> &'static RwLock<Arc<Vec<DictionaryEntry>>> {
    ACTIVE.get_or_init(|| RwLock::new(Arc::new(Vec::new())))
}

/// Entries that apply now. One `Arc` clone; safe to call on the paste path.
pub fn active_entries() -> Arc<Vec<DictionaryEntry>> {
    match active_cell().read() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

fn set_active_entries(entries: Vec<DictionaryEntry>) {
    let next = Arc::new(entries);
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
            seen_count: row.get("seen_count")?,
            created_at: row.get("created_at")?,
            updated_at: row.get("updated_at")?,
        })
    }

    const SELECT: &'static str = "SELECT id, wrong, right, case_mode, source, state, enabled, \
         seen_count, created_at, updated_at FROM dictionary";

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

    /// Insert or bump one pair inside an open transaction. Returns the row and
    /// whether it was new.
    fn upsert(
        conn: &Connection,
        entry: &DictionaryEntry,
        source: &str,
    ) -> Result<(DictionaryRow, bool)> {
        let wrong = entry.wrong.trim();
        let right = entry.right.trim();
        if right.is_empty() {
            return Err(anyhow!("right text is empty"));
        }
        let (wk, rk) = (key(wrong), key(right));
        let ts = now();
        Self::demote_other_active(conn, &wk, &rk)?;
        conn.execute(
            "INSERT INTO dictionary
               (wrong, right, match_mode, case_mode, source, state, enabled, seen_count,
                applied_count, app_id, wrong_key, right_key, created_at, updated_at)
             VALUES (?1, ?2, 'word', ?3, ?4, 'active', 1, 1, 0, '', ?5, ?6, ?7, ?7)
             ON CONFLICT(wrong_key, right_key, app_id) DO UPDATE SET
               seen_count = seen_count + 1,
               state = CASE WHEN state = 'rejected' THEN state ELSE 'active' END,
               updated_at = excluded.updated_at",
            params![
                wrong,
                right,
                case_mode_str(entry.case_mode),
                source,
                wk,
                rk,
                ts
            ],
        )?;
        let row = Self::get_by_keys(conn, &wk, &rk)?
            .ok_or_else(|| anyhow!("row missing after upsert"))?;
        let is_new = row.seen_count == 1 && row.created_at == ts;
        Ok((row, is_new))
    }

    /// Learn pairs from a producer. Returns what was new and what was known.
    pub fn learn(&self, entries: &[DictionaryEntry], source: &str) -> Result<LearnReport> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        let mut report = LearnReport {
            added: Vec::new(),
            known: Vec::new(),
        };
        for entry in entries {
            let (row, is_new) = Self::upsert(&tx, entry, source)?;
            if is_new {
                report.added.push(row);
            } else {
                report.known.push(row);
            }
        }
        tx.commit()?;
        self.notify();
        Ok(report)
    }

    /// Manual add from the Dictionary screen. Exact case: it is what the user
    /// typed. A pair that already exists is an error the screen reports.
    pub fn add_manual(&self, wrong: &str, right: &str) -> Result<DictionaryRow> {
        let (wrong, right) = (wrong.trim(), right.trim());
        if wrong.is_empty() || right.is_empty() {
            return Err(anyhow!("both texts are required"));
        }
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
        let (row, _) = Self::upsert(&tx, &entry, "manual")?;
        tx.commit()?;
        self.notify();
        Ok(row)
    }

    /// Edit a row in place. `active` maps to `state = 'active'` plus
    /// `enabled = 1`; false only clears `enabled`, so the row keeps its history.
    pub fn update(
        &self,
        id: i64,
        wrong: &str,
        right: &str,
        case_mode: CaseMode,
        active: bool,
    ) -> Result<DictionaryRow> {
        let (wrong, right) = (wrong.trim(), right.trim());
        if wrong.is_empty() || right.is_empty() {
            return Err(anyhow!("both texts are required"));
        }
        let (wk, rk) = (key(wrong), key(right));
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        if let Some(other) = Self::get_by_keys(&tx, &wk, &rk)? {
            if other.id != id {
                return Err(anyhow!("duplicate"));
            }
        }
        if active {
            Self::demote_other_active(&tx, &wk, &rk)?;
        }
        let changed = tx.execute(
            "UPDATE dictionary SET wrong = ?2, right = ?3, case_mode = ?4,
                wrong_key = ?5, right_key = ?6,
                state = CASE WHEN ?7 THEN 'active' ELSE state END,
                enabled = ?7, updated_at = ?8
             WHERE id = ?1",
            params![
                id,
                wrong,
                right,
                case_mode_str(case_mode),
                wk,
                rk,
                active,
                now()
            ],
        )?;
        if changed == 0 {
            return Err(anyhow!("entry {id} not found"));
        }
        let row = Self::get_by_keys(&tx, &wk, &rk)?.ok_or_else(|| anyhow!("row missing"))?;
        tx.commit()?;
        self.notify();
        Ok(row)
    }

    pub fn delete(&self, id: i64) -> Result<bool> {
        let conn = self.conn()?;
        let changed = conn.execute("DELETE FROM dictionary WHERE id = ?1", params![id])?;
        if changed > 0 {
            self.notify();
        }
        Ok(changed > 0)
    }

    /// Rebuild the hot-path snapshot from the table.
    pub fn refresh_cache(&self) -> Result<()> {
        let conn = self.conn()?;
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
        set_active_entries(entries);
        Ok(())
    }

    fn notify(&self) {
        if let Err(err) = self.refresh_cache() {
            warn!("dictionary: cache refresh failed: {err}");
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
            if let Err(err) = Self::upsert(&tx, entry, source) {
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
                created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
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

    #[test]
    fn upsert_bumps_seen_count_instead_of_duplicating() {
        let conn = fresh();
        let (a, new_a) =
            DictionaryManager::upsert(&conn, &entry("Bededa", "Pereira"), "capture").unwrap();
        let (b, new_b) =
            DictionaryManager::upsert(&conn, &entry("bededa", "Pereira"), "history").unwrap();
        assert!(new_a);
        assert!(!new_b);
        assert_eq!(a.id, b.id);
        assert_eq!(b.seen_count, 2);
        assert_eq!(b.source, "capture", "the first producer's source stays");
    }

    #[test]
    fn last_correction_wins_for_the_same_wrong_text() {
        let conn = fresh();
        let (old, _) = DictionaryManager::upsert(&conn, &entry("Maine", "main"), "manual").unwrap();
        let (new, _) =
            DictionaryManager::upsert(&conn, &entry("Maine", "Mane"), "history").unwrap();
        let old_now = conn
            .query_row(
                &format!("{} WHERE id = ?1", DictionaryManager::SELECT),
                params![old.id],
                DictionaryManager::map_row,
            )
            .unwrap();
        assert_eq!(old_now.state, "proposed");
        assert_eq!(new.state, "active");
    }

    #[test]
    fn rejected_rows_stay_rejected_on_repeat() {
        let conn = fresh();
        let (row, _) =
            DictionaryManager::upsert(&conn, &entry("we are", "we're"), "capture").unwrap();
        conn.execute(
            "UPDATE dictionary SET state = 'rejected' WHERE id = ?1",
            params![row.id],
        )
        .unwrap();
        let (again, is_new) =
            DictionaryManager::upsert(&conn, &entry("we are", "we're"), "capture").unwrap();
        assert!(!is_new);
        assert_eq!(again.state, "rejected");
        assert_eq!(again.seen_count, 2);
    }
}
