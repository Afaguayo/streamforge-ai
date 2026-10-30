//! VOD ingestion: watches the recording folder and accepts manual imports.
//!
//! A file is taken only once its size has stopped changing between two scans, so a
//! recording OBS is still writing is never picked up half-finished.

use crate::{db::Db, media::VIDEO_EXTS, settings::Settings};
use anyhow::{anyhow, Result};
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

/// Adds a recording to the library and queues it for probing.
/// If a Kick session ended and has no file yet, the recording is attached to it.
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
    let size = std::fs::metadata(path)?.len() as i64;
    let conn = db.conn();
    let awaiting: Option<i64> = conn
        .query_row(
            "SELECT id FROM streams WHERE source='kick' AND status='awaiting_file' ORDER BY id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .ok();
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
            .execute("INSERT INTO streams(source,title,status) VALUES('kick','Ranked','awaiting_file')", [])
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
    fn rejects_non_video() {
        let db = Db::memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.txt");
        std::fs::write(&f, b"x").unwrap();
        assert!(ingest_file(&db, &f).is_err());
    }
}
