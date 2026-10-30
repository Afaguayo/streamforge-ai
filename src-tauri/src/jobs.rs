//! Persistent job queue backed by SQLite.
//!
//! Jobs survive restarts: anything left `running` by a crash is put back to `queued`
//! on startup. Failures retry with exponential backoff until `max_attempts`.

use crate::db::Db;
use anyhow::Result;
use chrono::{Duration, Utc};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;

pub fn now_str() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

#[derive(Debug, Serialize, Clone)]
pub struct Job {
    pub id: i64,
    pub kind: String,
    pub stream_id: Option<i64>,
    pub payload: String,
    pub status: String,
    pub progress: f64,
    pub message: String,
    pub attempts: i64,
    pub max_attempts: i64,
    pub error: Option<String>,
    pub run_after: String,
    pub created_at: String,
    pub updated_at: String,
}

const JOB_COLS: &str =
    "id,kind,stream_id,payload,status,progress,message,attempts,max_attempts,error,run_after,created_at,updated_at";

fn job_from_row(r: &rusqlite::Row) -> rusqlite::Result<Job> {
    Ok(Job {
        id: r.get(0)?,
        kind: r.get(1)?,
        stream_id: r.get(2)?,
        payload: r.get(3)?,
        status: r.get(4)?,
        progress: r.get(5)?,
        message: r.get(6)?,
        attempts: r.get(7)?,
        max_attempts: r.get(8)?,
        error: r.get(9)?,
        run_after: r.get(10)?,
        created_at: r.get(11)?,
        updated_at: r.get(12)?,
    })
}

/// Seconds to wait before retry number `attempt` (1-based): 30s, 60s, 120s, … capped at 1h.
pub fn backoff_secs(attempt: i64) -> i64 {
    (30_i64 << (attempt - 1).clamp(0, 7)).min(3600)
}

impl Db {
    pub fn enqueue(&self, kind: &str, stream_id: Option<i64>, payload: &str) -> Result<i64> {
        let conn = self.conn();
        // Duplicate prevention: one live job per (kind, stream, payload).
        let existing: Option<i64> = conn
            .query_row(
                "SELECT id FROM jobs WHERE kind=?1 AND stream_id IS ?2 AND payload=?3 AND status IN ('queued','running')",
                params![kind, stream_id, payload],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = existing {
            return Ok(id);
        }
        conn.execute(
            "INSERT INTO jobs(kind,stream_id,payload,run_after) VALUES(?1,?2,?3,?4)",
            params![kind, stream_id, payload, now_str()],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Like `enqueue`, but not before `run_after` (UTC, `%Y-%m-%dT%H:%M:%SZ`).
    pub fn enqueue_at(&self, kind: &str, stream_id: Option<i64>, payload: &str, run_after: &str) -> Result<i64> {
        let id = self.enqueue(kind, stream_id, payload)?;
        self.conn().execute("UPDATE jobs SET run_after=?2 WHERE id=?1 AND status='queued'", params![id, run_after])?;
        Ok(id)
    }

    /// Called once at startup. Returns how many jobs were resumed.
    pub fn recover_interrupted(&self) -> Result<usize> {
        Ok(self.conn().execute(
            "UPDATE jobs SET status='queued', message='Resumed after restart', updated_at=?1 WHERE status='running'",
            [now_str()],
        )?)
    }

    pub fn claim_next(&self) -> Result<Option<Job>> {
        let conn = self.conn();
        let now = now_str();
        let job = conn
            .query_row(
                &format!("SELECT {JOB_COLS} FROM jobs WHERE status='queued' AND run_after<=?1 ORDER BY id LIMIT 1"),
                [&now],
                job_from_row,
            )
            .optional()?;
        let Some(mut job) = job else { return Ok(None) };
        conn.execute(
            "UPDATE jobs SET status='running', attempts=attempts+1, error=NULL, updated_at=?2 WHERE id=?1",
            params![job.id, now],
        )?;
        job.status = "running".into();
        job.attempts += 1;
        Ok(Some(job))
    }

    pub fn job_progress(&self, id: i64, progress: f64, message: &str) {
        let _ = self.conn().execute(
            "UPDATE jobs SET progress=?2, message=?3, updated_at=?4 WHERE id=?1",
            params![id, progress.clamp(0.0, 1.0), message, now_str()],
        );
    }

    pub fn job_done(&self, id: i64, message: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE jobs SET status='done', progress=1, message=?2, updated_at=?3 WHERE id=?1",
            params![id, message, now_str()],
        )?;
        Ok(())
    }

    /// Requeues with backoff, or marks failed once attempts are used up.
    pub fn job_failed(&self, job: &Job, error: &str) -> Result<()> {
        let conn = self.conn();
        if job.attempts < job.max_attempts {
            let at = (Utc::now() + Duration::seconds(backoff_secs(job.attempts)))
                .format("%Y-%m-%dT%H:%M:%SZ")
                .to_string();
            conn.execute(
                "UPDATE jobs SET status='queued', error=?2, run_after=?3, message='Waiting to retry', updated_at=?4 WHERE id=?1",
                params![job.id, error, at, now_str()],
            )?;
        } else {
            conn.execute(
                "UPDATE jobs SET status='failed', error=?2, updated_at=?3 WHERE id=?1",
                params![job.id, error, now_str()],
            )?;
        }
        Ok(())
    }

    pub fn job_retry(&self, id: i64) -> Result<bool> {
        Ok(self.conn().execute(
            "UPDATE jobs SET status='queued', attempts=0, error=NULL, progress=0, run_after=?2, updated_at=?2
             WHERE id=?1 AND status IN ('failed','cancelled')",
            params![id, now_str()],
        )? > 0)
    }

    pub fn job_cancel(&self, id: i64) -> Result<bool> {
        Ok(self.conn().execute(
            "UPDATE jobs SET status='cancelled', updated_at=?2 WHERE id=?1 AND status='queued'",
            params![id, now_str()],
        )? > 0)
    }

    pub fn list_jobs(&self, limit: u32) -> Result<Vec<Job>> {
        let conn = self.conn();
        let mut st = conn.prepare(&format!(
            "SELECT {JOB_COLS} FROM jobs ORDER BY CASE status WHEN 'running' THEN 0 WHEN 'queued' THEN 1 ELSE 2 END, id DESC LIMIT ?1"
        ))?;
        let rows = st.query_map([limit], job_from_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn job_counts(&self) -> Result<JobCounts> {
        let conn = self.conn();
        let count = |s: &str| -> rusqlite::Result<i64> {
            conn.query_row("SELECT COUNT(*) FROM jobs WHERE status=?1", [s], |r| r.get(0))
        };
        Ok(JobCounts { queued: count("queued")?, running: count("running")?, failed: count("failed")?, done: count("done")? })
    }
}

#[derive(Debug, Serialize)]
pub struct JobCounts {
    pub queued: i64,
    pub running: i64,
    pub failed: i64,
    pub done: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enqueue_deduplicates_live_jobs() {
        let db = Db::memory().unwrap();
        let a = db.enqueue("probe", None, "{}").unwrap();
        let b = db.enqueue("probe", None, "{}").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn crash_recovery_requeues_running_jobs() {
        let db = Db::memory().unwrap();
        db.enqueue("probe", None, "{}").unwrap();
        let job = db.claim_next().unwrap().unwrap();
        assert_eq!(job.status, "running");
        assert!(db.claim_next().unwrap().is_none(), "running job must not be claimed twice");
        assert_eq!(db.recover_interrupted().unwrap(), 1);
        assert!(db.claim_next().unwrap().is_some());
    }

    #[test]
    fn failure_retries_then_gives_up() {
        let db = Db::memory().unwrap();
        db.enqueue("probe", None, "{}").unwrap();
        let mut job = db.claim_next().unwrap().unwrap();
        job.max_attempts = 2;
        db.job_failed(&job, "boom").unwrap();
        let j = &db.list_jobs(10).unwrap()[0];
        assert_eq!(j.status, "queued");
        assert!(j.run_after > now_str(), "retry must be delayed");

        job.attempts = 2;
        db.job_failed(&job, "boom again").unwrap();
        let j = &db.list_jobs(10).unwrap()[0];
        assert_eq!(j.status, "failed");
        assert!(db.job_retry(j.id).unwrap());
        assert_eq!(db.list_jobs(10).unwrap()[0].status, "queued");
    }

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(backoff_secs(1), 30);
        assert_eq!(backoff_secs(2), 60);
        assert_eq!(backoff_secs(20), 3600);
    }
}
