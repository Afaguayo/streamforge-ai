//! VOD ingestion: watches the recording folder and accepts manual imports.
//!
//! A file is taken only once its size has stopped changing between two scans, so a
//! recording OBS is still writing is never picked up half-finished.

use crate::{db::Db, media::VIDEO_EXTS, settings::Settings};
use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const SCAN_EVERY: Duration = Duration::from_secs(20);

pub fn is_video(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| VIDEO_EXTS.contains(&e.to_ascii_lowercase().as_str()))
}

/// How long after a session ends its recording can finish writing.
const MATCH_GRACE_MIN: i64 = 20;

/// The session this file belongs to: it must have finished writing while that session ran
/// or just after it. "Newest waiting session" isn't enough, because a stream with no
/// recording would then take the next stream's file. A session with no end is still live:
/// OBS can finish the file a minute or two before Kick reports the stream offline.
fn session_for(sessions: &[(i64, Option<String>, Option<String>)], written: DateTime<Utc>) -> Option<i64> {
    sessions.iter().find_map(|(id, started, ended)| {
        let t = |v: &Option<String>| DateTime::parse_from_rfc3339(v.as_deref()?).ok().map(|d| d.with_timezone(&Utc));
        let a = t(started)?;
        let b = t(ended).unwrap_or_else(Utc::now);
        (written >= a && written <= b + chrono::Duration::minutes(MATCH_GRACE_MIN)).then_some(*id)
    })
}

/// When a file was last written.
fn written(path: &str) -> Option<DateTime<Utc>> {
    std::fs::metadata(path).ok()?.modified().ok().map(Into::into)
}

/// Gives a session the recording that was imported on its own before the session could
/// claim it (from an older version, or a hand import). The recording's entry takes the
/// session's title, times and upload choice, and the empty session entry goes. Returns the
/// recording's stream id.
pub fn adopt_recording(db: &Db, session: i64) -> Result<Option<i64>> {
    let (started, ended): (Option<String>, Option<String>) =
        db.conn().query_row("SELECT kick_started,kick_ended FROM streams WHERE id=?1", [session], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let locals: Vec<(i64, String)> = {
        let conn = db.conn();
        let mut st = conn.prepare("SELECT id,file_path FROM streams WHERE source='local' AND file_path IS NOT NULL")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<Result<_, _>>()?
    };
    let one = [(session, started, ended)];
    let Some((local, _)) = locals.into_iter().find(|(_, p)| written(p).is_some_and(|w| session_for(&one, w).is_some())) else { return Ok(None) };
    let conn = db.conn();
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "UPDATE streams SET source='kick', title=s.title, kick_started=s.kick_started, kick_ended=s.kick_ended, auto_upload=s.auto_upload
         FROM (SELECT title,kick_started,kick_ended,auto_upload FROM streams WHERE id=?2) s WHERE streams.id=?1",
        rusqlite::params![local, session],
    )?;
    tx.execute("DELETE FROM streams WHERE id=?1", [session])?;
    tx.commit()?;
    Ok(Some(local))
}

/// Adds a recording to the library and queues it for probing.
/// If it was written during a Kick session that has no file yet, it's attached to that session.
pub fn ingest_file(db: &Db, path: &Path) -> Result<i64> {
    if !path.is_file() {
        return Err(anyhow!("not a file: {}", path.display()));
    }
    if !is_video(path) {
        return Err(anyhow!("unsupported file type: {}", path.display()));
    }
    let path_str = path.to_string_lossy().to_string();
    if let Some(id) = db.stream_by_path(&path_str)? {
        return Ok(id);
    }
    let meta = std::fs::metadata(path)?;
    let size = meta.len() as i64;
    let written: DateTime<Utc> = meta.modified()?.into();
    let conn = db.conn();
    let sessions: Vec<(i64, Option<String>, Option<String>)> = {
        let mut st = conn.prepare("SELECT id,kick_started,kick_ended FROM streams WHERE source='kick' AND status IN ('awaiting_file','live') ORDER BY id DESC")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect::<Result<_, _>>()?
    };
    let awaiting = session_for(&sessions, written);
    let id = match awaiting {
        Some(id) => {
            conn.execute(
                "UPDATE streams SET file_path=?2, file_size=?3, status='queued' WHERE id=?1",
                rusqlite::params![id, path_str, size],
            )?;
            id
        }
        None => {
            let title = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            conn.execute(
                "INSERT INTO streams(source,title,file_path,file_size,status) VALUES('local',?1,?2,?3,'queued')",
                rusqlite::params![title, path_str, size],
            )?;
            conn.last_insert_rowid()
        }
    };
    drop(conn);
    db.enqueue("probe", Some(id), "{}")?;
    let how = if awaiting.is_some() { "matched to the Kick stream that just ended" } else { "added" };
    db.log("info", "ingest", &format!("Recording {how}: {}", path.display()));
    Ok(id)
}

/// Returns files whose size matched the previous scan (finished writing).
pub fn stable_files(dir: &Path, prev: &mut HashMap<PathBuf, u64>) -> Vec<PathBuf> {
    let mut seen = HashMap::new();
    let mut stable = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return stable };
    for e in entries.flatten() {
        let p = e.path();
        if !is_video(&p) {
            continue;
        }
        let Ok(meta) = e.metadata() else { continue };
        let size = meta.len();
        if size > 0 && prev.get(&p) == Some(&size) {
            stable.push(p.clone());
        }
        seen.insert(p, size);
    }
    *prev = seen;
    stable
}

pub async fn watch_loop(db: Arc<Db>) {
    let mut sizes = HashMap::new();
    let mut last_dir: Option<String> = None;
    loop {
        let dir = Settings::load(&db).recording_dir;
        if dir != last_dir {
            sizes.clear();
            last_dir = dir.clone();
        }
        if let Some(dir) = dir {
            for p in stable_files(Path::new(&dir), &mut sizes) {
                if db.stream_by_path(&p.to_string_lossy()).ok().flatten().is_some() {
                    continue;
                }
                if let Err(e) = ingest_file(&db, &p) {
                    db.log("error", "ingest", &format!("{}: {e:#}", p.display()));
                }
            }
        }
        tokio::time::sleep(SCAN_EVERY).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn growing_file_is_not_stable_until_it_stops() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("rec.mkv");
        std::fs::write(&f, b"abc").unwrap();
        std::fs::write(dir.path().join("notes.txt"), b"x").unwrap();
        let mut prev = HashMap::new();
        assert!(stable_files(dir.path(), &mut prev).is_empty(), "first sighting");
        std::fs::write(&f, b"abcdef").unwrap();
        assert!(stable_files(dir.path(), &mut prev).is_empty(), "still growing");
        assert_eq!(stable_files(dir.path(), &mut prev), vec![f]);
    }

    #[test]
    fn ingest_attaches_to_ended_kick_session_and_dedupes() {
        let db = Db::memory().unwrap();
        db.conn()
            .execute(
                "INSERT INTO streams(source,title,status,kick_started,kick_ended) VALUES('kick','Ranked','awaiting_file',?1,?2)",
                [(Utc::now() - chrono::Duration::hours(2)).to_rfc3339(), Utc::now().to_rfc3339()],
            )
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("2026-10-29 01-00-00.mp4");
        std::fs::write(&f, b"data").unwrap();
        let id = ingest_file(&db, &f).unwrap();
        assert_eq!(ingest_file(&db, &f).unwrap(), id);
        let s = db.get_stream(id).unwrap().unwrap();
        assert_eq!((s.source.as_str(), s.title.as_str(), s.status.as_str()), ("kick", "Ranked", "queued"));
        assert_eq!(db.job_counts().unwrap().queued, 1);
    }

    #[test]
    fn a_recording_goes_to_the_session_it_was_written_in() {
        let t = |s: &str| Some(s.to_string());
        let sessions = vec![
            (4, t("2026-11-06T01:47:03Z"), t("2026-11-06T05:55:13Z")),
            (3, t("2026-11-05T23:00:27Z"), t("2026-11-06T00:58:50Z")),
            (2, t("2026-10-31T01:48:03Z"), t("2026-10-31T01:49:54Z")),
        ];
        let at = |s: &str| DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc);
        assert_eq!(session_for(&sessions, at("2026-11-06T00:56:00Z")), Some(3));
        assert_eq!(session_for(&sessions, at("2026-11-06T05:53:00Z")), Some(4));
        assert_eq!(session_for(&sessions, at("2026-11-06T12:00:00Z")), None, "unrelated files stay separate");
        let live = vec![(5, t("2026-11-07T21:32:32Z"), None)];
        assert_eq!(session_for(&live, at("2026-11-07T23:37:09Z")), Some(5), "file done before Kick says offline");
    }

    #[test]
    fn a_session_takes_over_a_recording_imported_on_its_own() {
        let db = Db::memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("2026-11-07 15-32-25.mp4");
        std::fs::write(&f, b"data").unwrap();
        let (start, end) = ((Utc::now() - chrono::Duration::hours(2)).to_rfc3339(), Utc::now().to_rfc3339());
        db.conn().execute("INSERT INTO streams(source,title,file_path,status) VALUES('local','2026-11-07 15-32-25',?1,'probed')", [f.to_string_lossy()]).unwrap();
        db.conn()
            .execute("INSERT INTO streams(source,title,status,kick_started,kick_ended,auto_upload) VALUES('kick','Love story 3','awaiting_file',?1,?2,1)", [&start, &end])
            .unwrap();
        assert_eq!(adopt_recording(&db, 2).unwrap(), Some(1));
        let s = db.get_stream(1).unwrap().unwrap();
        assert_eq!((s.source.as_str(), s.title.as_str(), s.status.as_str()), ("kick", "Love story 3", "probed"));
        assert!(db.get_stream(2).unwrap().is_none(), "the empty session entry is gone");
        let up: i64 = db.conn().query_row("SELECT auto_upload FROM streams WHERE id=1", [], |r| r.get(0)).unwrap();
        assert_eq!(up, 1);
    }

    #[test]
    fn a_recording_finished_while_live_is_attached() {
        let db = Db::memory().unwrap();
        db.conn()
            .execute("INSERT INTO streams(source,title,status,kick_started) VALUES('kick','Live now','live',?1)", [(Utc::now() - chrono::Duration::hours(2)).to_rfc3339()])
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("rec.mp4");
        std::fs::write(&f, b"data").unwrap();
        assert_eq!(ingest_file(&db, &f).unwrap(), 1);
        assert_eq!(db.list_streams().unwrap().len(), 1);
    }

    #[test]
    fn rejects_non_video() {
        let db = Db::memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.txt");
        std::fs::write(&f, b"x").unwrap();
        assert!(ingest_file(&db, &f).is_err());
    }
}
