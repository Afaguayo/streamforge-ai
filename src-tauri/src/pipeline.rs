//! Job worker: claims one job at a time from the queue and runs its handler.
//! One at a time on purpose, so processing never takes over the PC while gaming.

use crate::{db::Db, jobs::Job, kick::KickState, media, settings::Settings};
use anyhow::{anyhow, Result};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Jobs that load the GPU/CPU hard; held back while live if the user asks.
fn is_heavy(kind: &str) -> bool {
    !matches!(kind, "probe")
}

pub async fn worker_loop(db: Arc<Db>, kick: Arc<Mutex<KickState>>) {
    loop {
        let live = kick.lock().unwrap().live;
        let hold_heavy = live && Settings::load(&db).pause_while_live;
        match db.claim_next() {
            Ok(Some(job)) if hold_heavy && is_heavy(&job.kind) => {
                // Put it back untouched (claim counted an attempt; undo that).
                let _ = db.conn().execute(
                    "UPDATE jobs SET status='queued', attempts=attempts-1, message='Paused while you are live' WHERE id=?1",
                    [job.id],
                );
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
            Ok(Some(job)) => {
                let result = run(&db, &job).await;
                match result {
                    Ok(msg) => {
                        let _ = db.job_done(job.id, &msg);
                    }
                    Err(e) => {
                        let err = format!("{e:#}");
                        db.log("error", &job.kind, &format!("Job #{} failed (attempt {}): {err}", job.id, job.attempts));
                        let _ = db.job_failed(&job, &err);
                        if let Some(sid) = job.stream_id {
                            if job.attempts >= job.max_attempts {
                                let _ = db.conn().execute(
                                    "UPDATE streams SET status='error', error=?2 WHERE id=?1",
                                    rusqlite::params![sid, err],
                                );
                            }
                        }
                    }
                }
            }
            Ok(None) => tokio::time::sleep(Duration::from_secs(2)).await,
            Err(e) => {
                db.log("error", "queue", &format!("Queue error: {e:#}"));
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        }
    }
}

async fn run(db: &Db, job: &Job) -> Result<String> {
    match job.kind.as_str() {
        "probe" => probe(db, job).await,
        other => Err(anyhow!("no handler for job kind '{other}' yet")),
    }
}

async fn probe(db: &Db, job: &Job) -> Result<String> {
    let sid = job.stream_id.ok_or_else(|| anyhow!("probe job has no stream"))?;
    let stream = db.get_stream(sid)?.ok_or_else(|| anyhow!("stream #{sid} no longer exists"))?;
    let path = stream.file_path.ok_or_else(|| anyhow!("stream #{sid} has no recording file"))?;
    db.job_progress(job.id, 0.1, "Reading video metadata");
    let info = media::probe(&path).await?;
    db.conn().execute(
        "UPDATE streams SET duration_s=?2, file_size=COALESCE(?3,file_size), width=?4, height=?5, fps=?6,
         video_codec=?7, audio_codec=?8, status='probed', error=NULL WHERE id=?1",
        rusqlite::params![sid, info.duration_s, info.size, info.width, info.height, info.fps, info.video_codec, info.audio_codec],
    )?;
    let len = hms(info.duration_s.unwrap_or(0.0));
    db.log("info", "probe", &format!("{}: {len}, {}x{}", stream.title, info.width.unwrap_or(0), info.height.unwrap_or(0)));
    Ok(format!("{len} recording ready for analysis"))
}

fn hms(secs: f64) -> String {
    let s = secs.round() as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

#[cfg(test)]
mod tests {
    #[test]
    fn hms_formats() {
        assert_eq!(super::hms(12.02), "0:12");
        assert_eq!(super::hms(7265.0), "2:01:05");
    }
}
