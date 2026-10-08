//! The Dictionary database, `dictionary.db`, next to `history.db`.
//!
//! The Dictionary has its own file so that `history.db` keeps upstream's
//! schema. Upstream Handy refuses to open a database with a `user_version`
//! higher than its own migration count. If a person installs upstream Handy
//! over a fork build, it must still start; it only loses the Dictionary.
//!
//! Fork builds up to 0.9.8-katapult.1 kept the `dictionary` table in
//! `history.db` as migrations 5 to 7. [`move_out_of_history`] moves that table
//! here once and sets `history.db` back to upstream's version 4.
//!
//! Rule for upstream merges: never add a migration to `managers/history.rs`.
//! Dictionary schema changes go in [`MIGRATIONS`] in this file.

use anyhow::Result;
use log::info;
use rusqlite::{Connection, OptionalExtension};
use rusqlite_migration::{Migrations, M};
use std::path::Path;

pub const FILE_NAME: &str = "dictionary.db";

/// Upstream's `history.db` version when the fork added migrations 5 to 7.
/// A `history.db` that still holds the `dictionary` table is set back to
/// this version after the move.
const UPSTREAM_HISTORY_VERSION: i32 = 4;

// Dictionary entries (see docs/DICTIONARY_DESIGN.md section 5). Keys are
// normalized copies for uniqueness; `''` sentinels, never NULL, so the
// unique index and the UPSERT behave. `dictionary_active_wrong` enforces
// one active replacement per wrong text. Owned by dictionary_store.rs.
const CREATE_TABLE: &str = "CREATE TABLE IF NOT EXISTS dictionary (
        id            INTEGER PRIMARY KEY AUTOINCREMENT,
        wrong         TEXT NOT NULL DEFAULT '',
        right         TEXT NOT NULL,
        match_mode    TEXT NOT NULL DEFAULT 'word',
        case_mode     TEXT NOT NULL DEFAULT 'smart',
        source        TEXT NOT NULL,
        state         TEXT NOT NULL DEFAULT 'active',
        enabled       INTEGER NOT NULL DEFAULT 1,
        seen_count    INTEGER NOT NULL DEFAULT 1,
        applied_count INTEGER NOT NULL DEFAULT 0,
        app_id        TEXT NOT NULL DEFAULT '',
        wrong_key     TEXT NOT NULL,
        right_key     TEXT NOT NULL,
        created_at    INTEGER NOT NULL,
        updated_at    INTEGER NOT NULL
    );
    CREATE UNIQUE INDEX IF NOT EXISTS dictionary_pair
        ON dictionary (wrong_key, right_key, app_id);
    CREATE UNIQUE INDEX IF NOT EXISTS dictionary_active_wrong
        ON dictionary (wrong_key, app_id)
        WHERE state = 'active' AND wrong_key != '';";

// Early SQLite builds activated every learned row immediately. Require a
// fresh user confirmation for those rows without changing manual entries,
// rejected rows, or entries the user already disabled.
const DEMOTE_LEARNED: &str = "UPDATE dictionary
     SET state = 'proposed'
     WHERE source IN ('history', 'capture')
       AND state = 'active'
       AND enabled = 1;";

// Conservative automatic learning. `auto_learned` distinguishes rows
// activated by the classifier from explicit user decisions. Context is a
// JSON array and is required at match time for automatic rows.
const ADD_AUTO_LEARNING: &str = "ALTER TABLE dictionary
         ADD COLUMN auto_learned INTEGER NOT NULL DEFAULT 0;
     ALTER TABLE dictionary
         ADD COLUMN context_words TEXT NOT NULL DEFAULT '[]';";

static MIGRATIONS: &[M] = &[
    M::up(CREATE_TABLE),
    M::up(DEMOTE_LEARNED),
    M::up(ADD_AUTO_LEARNING),
];

/// Every column, in table order. The move copies them by name.
const COLUMNS: &str = "wrong, right, match_mode, case_mode, source, state, enabled, \
     seen_count, applied_count, app_id, wrong_key, right_key, created_at, updated_at, \
     auto_learned, context_words";

/// Opens `dictionary.db` and applies pending migrations.
pub fn open(path: &Path) -> Result<Connection> {
    let mut conn = Connection::open(path)?;
    migrate(&mut conn)?;
    Ok(conn)
}

fn migrate(conn: &mut Connection) -> Result<()> {
    let migrations = Migrations::new(MIGRATIONS.to_vec());
    #[cfg(debug_assertions)]
    migrations
        .validate()
        .expect("Invalid dictionary migrations");
    migrations.to_latest(conn)?;
    Ok(())
}

fn has_table(conn: &Connection, schema: &str, table: &str) -> Result<bool> {
    let found = conn
        .query_row(
            &format!("SELECT 1 FROM {schema}.sqlite_master WHERE type = 'table' AND name = ?1"),
            [table],
            |_| Ok(()),
        )
        .optional()?;
    Ok(found.is_some())
}

fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for name in names {
        if name? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Moves a `dictionary` table left in `history.db` by an older fork build
/// into `dictionary.db`, then sets `history.db` back to upstream's version.
///
/// Call this before the history migrations run: with the fork versions still
/// set, they fail as "database too far ahead". Safe to run again after a
/// crash: the first step copies, the second step drops, and a repeated copy
/// skips pairs that are already there.
pub fn move_out_of_history(history: &mut Connection, dictionary_path: &Path) -> Result<()> {
    if !has_table(history, "main", "dictionary")? {
        return Ok(());
    }
    let version: i32 = history.pragma_query_value(None, "user_version", |row| row.get(0))?;
    info!("dictionary: moving the table out of history.db (version {version})");

    // Bring the old table to the current shape. Fork migration 6 demoted
    // learned rows; it ran only on databases that reached version 6.
    if version <= 5 {
        history.execute_batch(DEMOTE_LEARNED)?;
    }
    if !has_column(history, "dictionary", "auto_learned")? {
        history.execute_batch(ADD_AUTO_LEARNING)?;
    }

    // Create the destination with its own migrations, then copy. Keep the ids
    // when the destination is empty (the normal case). Otherwise let it assign
    // new ids, and the pair index skips rows it already has.
    drop(open(dictionary_path)?);
    history.execute(
        "ATTACH DATABASE ?1 AS dict",
        [dictionary_path.to_string_lossy()],
    )?;
    let result = (|| -> Result<()> {
        let tx = history.transaction()?;
        let empty: bool = tx.query_row(
            "SELECT NOT EXISTS (SELECT 1 FROM dict.dictionary)",
            [],
            |row| row.get(0),
        )?;
        let copied = if empty {
            tx.execute(
                &format!(
                    "INSERT INTO dict.dictionary (id, {COLUMNS})
                     SELECT id, {COLUMNS} FROM main.dictionary"
                ),
                [],
            )?
        } else {
            tx.execute(
                &format!(
                    "INSERT OR IGNORE INTO dict.dictionary ({COLUMNS})
                     SELECT {COLUMNS} FROM main.dictionary ORDER BY id"
                ),
                [],
            )?
        };
        tx.commit()?;
        info!("dictionary: copied {copied} entries to {FILE_NAME}");

        // Only after the copy is committed: remove the table and give
        // history.db upstream's version.
        let tx = history.transaction()?;
        tx.execute_batch(&format!(
            "DROP TABLE main.dictionary;
             PRAGMA main.user_version = {UPSTREAM_HISTORY_VERSION};"
        ))?;
        tx.commit()?;
        Ok(())
    })();
    history.execute("DETACH DATABASE dict", [])?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    /// Upstream's history migrations at the time of the fork. Kept here so a
    /// test can stand in for upstream Handy opening the database.
    const UPSTREAM_HISTORY: [&str; 4] = [
        "CREATE TABLE IF NOT EXISTS transcription_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            file_name TEXT NOT NULL,
            timestamp INTEGER NOT NULL,
            saved BOOLEAN NOT NULL DEFAULT 0,
            title TEXT NOT NULL,
            transcription_text TEXT NOT NULL
        );",
        "ALTER TABLE transcription_history ADD COLUMN post_processed_text TEXT;",
        "ALTER TABLE transcription_history ADD COLUMN post_process_prompt TEXT;",
        "ALTER TABLE transcription_history ADD COLUMN post_process_requested BOOLEAN NOT NULL DEFAULT 0;",
    ];

    fn upstream_migrations() -> Migrations<'static> {
        Migrations::new(UPSTREAM_HISTORY.iter().map(|sql| M::up(sql)).collect())
    }

    /// The history.db list of fork builds up to 0.9.8-katapult.1, cut at
    /// `version`.
    fn legacy_history(version: usize) -> Connection {
        let mut all: Vec<M> = UPSTREAM_HISTORY.iter().map(|sql| M::up(sql)).collect();
        all.extend(MIGRATIONS.iter().cloned());
        all.truncate(version);
        let mut conn = Connection::open_in_memory().unwrap();
        Migrations::new(all).to_latest(&mut conn).unwrap();
        conn.execute(
            "INSERT INTO transcription_history (file_name, timestamp, title, transcription_text)
             VALUES ('a.wav', 1, 'A', 'hello maine')",
            [],
        )
        .unwrap();
        conn
    }

    fn insert_legacy_rows(conn: &Connection) {
        let rows = [
            ("manual", "active", 1, "manual"),
            ("history", "active", 1, "history"),
            ("capture", "active", 1, "capture"),
            ("capture-disabled", "active", 0, "capture"),
            ("history-rejected", "rejected", 1, "history"),
        ];
        for (wrong, state, enabled, source) in rows {
            conn.execute(
                "INSERT INTO dictionary
                   (wrong, right, source, state, enabled, wrong_key, right_key,
                    created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?1, ?2, 1, 1)",
                params![wrong, format!("{wrong}-right"), source, state, enabled],
            )
            .unwrap();
        }
    }

    fn version(conn: &Connection) -> i32 {
        conn.pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap()
    }

    fn state(conn: &Connection, wrong: &str) -> String {
        conn.query_row(
            "SELECT state FROM dictionary WHERE wrong = ?1",
            [wrong],
            |row| row.get(0),
        )
        .unwrap()
    }

    fn temp_dictionary() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        (dir, path)
    }

    #[test]
    fn fresh_database_gets_the_current_schema() {
        let (_dir, path) = temp_dictionary();
        let conn = open(&path).unwrap();
        assert!(has_column(&conn, "dictionary", "context_words").unwrap());
        assert_eq!(version(&conn), MIGRATIONS.len() as i32);
    }

    #[test]
    fn history_without_dictionary_is_left_alone() {
        let (_dir, path) = temp_dictionary();
        let mut history = Connection::open_in_memory().unwrap();
        upstream_migrations().to_latest(&mut history).unwrap();
        move_out_of_history(&mut history, &path).unwrap();
        assert_eq!(version(&history), 4);
        assert!(!path.exists());
    }

    #[test]
    fn version_7_moves_rows_and_upstream_can_open_history() {
        let (_dir, path) = temp_dictionary();
        let mut history = legacy_history(7);
        conn_insert_auto_row(&history);
        move_out_of_history(&mut history, &path).unwrap();

        assert_eq!(version(&history), 4);
        assert!(!has_table(&history, "main", "dictionary").unwrap());
        // Upstream Handy's own migration run: no "too far ahead" error.
        upstream_migrations().to_latest(&mut history).unwrap();
        let history_rows: i64 = history
            .query_row("SELECT COUNT(*) FROM transcription_history", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(history_rows, 1);

        let dict = open(&path).unwrap();
        let (id, auto, context): (i64, i64, String) = dict
            .query_row(
                "SELECT id, auto_learned, context_words FROM dictionary WHERE wrong = 'maine'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((id, auto, context.as_str()), (1, 1, "[\"cove\"]"));
    }

    fn conn_insert_auto_row(conn: &Connection) {
        conn.execute(
            "INSERT INTO dictionary
               (wrong, right, source, state, enabled, wrong_key, right_key,
                created_at, updated_at, auto_learned, context_words)
             VALUES ('maine', 'main', 'capture', 'active', 1, 'maine', 'main',
                     1, 1, 1, '[\"cove\"]')",
            [],
        )
        .unwrap();
    }

    #[test]
    fn version_5_demotes_learned_rows_and_adds_columns() {
        let (_dir, path) = temp_dictionary();
        let mut history = legacy_history(5);
        insert_legacy_rows(&history);
        move_out_of_history(&mut history, &path).unwrap();

        let dict = open(&path).unwrap();
        assert_eq!(state(&dict, "manual"), "active");
        assert_eq!(state(&dict, "history"), "proposed");
        assert_eq!(state(&dict, "capture"), "proposed");
        assert_eq!(state(&dict, "capture-disabled"), "active");
        assert_eq!(state(&dict, "history-rejected"), "rejected");
        let defaults: (i64, String) = dict
            .query_row(
                "SELECT auto_learned, context_words FROM dictionary WHERE wrong = 'manual'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(defaults, (0, "[]".to_string()));
        assert_eq!(version(&history), 4);
    }

    #[test]
    fn version_6_keeps_states_and_adds_columns() {
        let (_dir, path) = temp_dictionary();
        let mut history = legacy_history(6);
        insert_legacy_rows(&history);
        move_out_of_history(&mut history, &path).unwrap();

        let dict = open(&path).unwrap();
        // Version 6 already ran the demotion; rows written after it stay.
        assert_eq!(state(&dict, "history"), "active");
        assert!(has_column(&dict, "dictionary", "auto_learned").unwrap());
    }

    #[test]
    fn repeated_move_after_a_crash_does_not_duplicate() {
        let (_dir, path) = temp_dictionary();
        let mut history = legacy_history(7);
        insert_legacy_rows(&history);
        // A crash after the copy but before the drop: the rows are in both.
        {
            let dict = open(&path).unwrap();
            dict.execute(
                "INSERT INTO dictionary (wrong, right, source, wrong_key, right_key,
                    created_at, updated_at)
                 VALUES ('manual', 'manual-right', 'manual', 'manual', 'manual-right', 1, 1)",
                [],
            )
            .unwrap();
        }
        move_out_of_history(&mut history, &path).unwrap();

        let dict = open(&path).unwrap();
        let count: i64 = dict
            .query_row("SELECT COUNT(*) FROM dictionary", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 5);
        assert_eq!(version(&history), 4);
    }

    #[test]
    fn later_upstream_history_migration_still_runs() {
        let (_dir, path) = temp_dictionary();
        let mut history = legacy_history(7);
        move_out_of_history(&mut history, &path).unwrap();

        // Upstream adds a fifth history migration in a later release.
        let mut next: Vec<M> = UPSTREAM_HISTORY.iter().map(|sql| M::up(sql)).collect();
        next.push(M::up(
            "ALTER TABLE transcription_history ADD COLUMN future TEXT;",
        ));
        Migrations::new(next).to_latest(&mut history).unwrap();
        assert!(has_column(&history, "transcription_history", "future").unwrap());
    }
}
