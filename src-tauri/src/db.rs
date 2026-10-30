//! SQLite storage: schema migrations, settings, streams and the activity log.
//! Job-queue tables live here too; queue logic is in `jobs.rs`.

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

/// Each entry runs once, in order; `PRAGMA user_version` records how many ran.
const MIGRATIONS: &[&str] = &[
    // 1: core tables
    r#"
    CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);

    CREATE TABLE streams (
        id            INTEGER PRIMARY KEY,
        source        TEXT NOT NULL,              -- 'local' | 'kick'
        title         TEXT NOT NULL,
        kick_started  TEXT,                       -- Kick-reported start time (ISO-8601)
        kick_ended    TEXT,
        file_path     TEXT UNIQUE,
        file_size     INTEGER,
        duration_s    REAL,
        width         INTEGER,
        height        INTEGER,
        fps           REAL,
        video_codec   TEXT,
        audio_codec   TEXT,
        status        TEXT NOT NULL,              -- 'live' | 'awaiting_file' | 'queued' | 'probed' | 'error'
        error         TEXT,
        created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now'))
    );

    CREATE TABLE jobs (
        id            INTEGER PRIMARY KEY,
        kind          TEXT NOT NULL,
        stream_id     INTEGER REFERENCES streams(id) ON DELETE CASCADE,
        payload       TEXT NOT NULL DEFAULT '{}',
        status        TEXT NOT NULL DEFAULT 'queued', -- queued | running | done | failed | cancelled
        progress      REAL NOT NULL DEFAULT 0,
        message       TEXT NOT NULL DEFAULT '',
        attempts      INTEGER NOT NULL DEFAULT 0,
        max_attempts  INTEGER NOT NULL DEFAULT 3,
        error         TEXT,
        run_after     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now')),
        created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now')),
        updated_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now'))
    );
    CREATE INDEX jobs_pick ON jobs(status, run_after);

    CREATE TABLE activity (
        id      INTEGER PRIMARY KEY,
        ts      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now')),
        level   TEXT NOT NULL,                     -- info | warn | error
        source  TEXT NOT NULL,
        message TEXT NOT NULL
    );
    "#,
    // 2: connected social accounts (tokens themselves live in Credential Manager)
    r#"
    CREATE TABLE accounts (
        platform      TEXT PRIMARY KEY,           -- kick | youtube | tiktok | meta
        display_name  TEXT NOT NULL,
        account_id    TEXT,
        details       TEXT NOT NULL DEFAULT '{}', -- e.g. Kick slug, Facebook Pages + Instagram accounts
        connected_at  TEXT NOT NULL,
        expires_at    TEXT,                       -- when sign-in must be redone; NULL = unknown/none
        last_check    TEXT,
        last_error    TEXT
    );
    "#,
];

pub struct Db(Mutex<Connection>);

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    #[cfg(test)]
    pub fn memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;")?;
        let version: usize = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(version) {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(sql)?;
            tx.execute_batch(&format!("PRAGMA user_version = {}", i + 1))?;
            tx.commit()?;
        }
        Ok(Db(Mutex::new(conn)))
    }

    pub fn conn(&self) -> MutexGuard<'_, Connection> {
        // A panic while holding the lock leaves the connection usable; recover it.
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ---- settings ----

    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn()
            .query_row("SELECT value FROM settings WHERE key=?1", [key], |r| r.get(0))
            .optional()?)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn().execute(
            "INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    // ---- activity log ----

    pub fn log(&self, level: &str, source: &str, message: &str) {
        let _ = self.conn().execute(
            "INSERT INTO activity(level,source,message) VALUES(?1,?2,?3)",
            params![level, source, message],
        );
    }

    pub fn recent_activity(&self, limit: u32) -> Result<Vec<Activity>> {
        let conn = self.conn();
        let mut st = conn.prepare("SELECT id,ts,level,source,message FROM activity ORDER BY id DESC LIMIT ?1")?;
        let rows = st.query_map([limit], |r| {
            Ok(Activity { id: r.get(0)?, ts: r.get(1)?, level: r.get(2)?, source: r.get(3)?, message: r.get(4)? })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    // ---- streams ----

    pub fn list_streams(&self) -> Result<Vec<Stream>> {
        let conn = self.conn();
        let mut st = conn.prepare(&format!("SELECT {STREAM_COLS} FROM streams ORDER BY id DESC"))?;
        let rows = st.query_map([], Stream::from_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn get_stream(&self, id: i64) -> Result<Option<Stream>> {
        Ok(self
            .conn()
            .query_row(&format!("SELECT {STREAM_COLS} FROM streams WHERE id=?1"), [id], Stream::from_row)
            .optional()?)
    }

    pub fn stream_by_path(&self, path: &str) -> Result<Option<i64>> {
        Ok(self
            .conn()
            .query_row("SELECT id FROM streams WHERE file_path=?1", [path], |r| r.get(0))
            .optional()?)
    }
}

const STREAM_COLS: &str = "id,source,title,kick_started,kick_ended,file_path,file_size,duration_s,width,height,fps,video_codec,audio_codec,status,error,created_at";

#[derive(Debug, Serialize, Clone)]
pub struct Stream {
    pub id: i64,
    pub source: String,
    pub title: String,
    pub kick_started: Option<String>,
    pub kick_ended: Option<String>,
    pub file_path: Option<String>,
    pub file_size: Option<i64>,
    pub duration_s: Option<f64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub fps: Option<f64>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub status: String,
    pub error: Option<String>,
    pub created_at: String,
}

impl Stream {
    fn from_row(r: &rusqlite::Row) -> rusqlite::Result<Self> {
        Ok(Stream {
            id: r.get(0)?,
            source: r.get(1)?,
            title: r.get(2)?,
            kick_started: r.get(3)?,
            kick_ended: r.get(4)?,
            file_path: r.get(5)?,
            file_size: r.get(6)?,
            duration_s: r.get(7)?,
            width: r.get(8)?,
            height: r.get(9)?,
            fps: r.get(10)?,
            video_codec: r.get(11)?,
            audio_codec: r.get(12)?,
            status: r.get(13)?,
            error: r.get(14)?,
            created_at: r.get(15)?,
        })
    }
}

#[derive(Debug, Serialize)]
pub struct Activity {
    pub id: i64,
    pub ts: String,
    pub level: String,
    pub source: String,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_idempotent_on_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        Db::open(&path).unwrap().set_setting("a", "1").unwrap();
        let db = Db::open(&path).unwrap();
        assert_eq!(db.get_setting("a").unwrap().as_deref(), Some("1"));
        let v: usize = db.conn().query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, MIGRATIONS.len());
    }

    #[test]
    fn settings_upsert() {
        let db = Db::memory().unwrap();
        db.set_setting("k", "x").unwrap();
        db.set_setting("k", "y").unwrap();
        assert_eq!(db.get_setting("k").unwrap().as_deref(), Some("y"));
        assert_eq!(db.get_setting("missing").unwrap(), None);
    }
}
