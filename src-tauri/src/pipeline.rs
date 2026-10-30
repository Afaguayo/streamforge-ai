//! Job worker: claims one job at a time from the queue and runs its handler.
//! One at a time on purpose, so processing never takes over the PC while gaming.
//!
//! Recording flow: probe → transcribe → analyze → (render top clips, thumbnails)
//! → you approve → publish. Scheduled jobs: stats, trend report, watchlist.

use crate::{analysis, db::Db, jobs::Job, kick::KickState, llm, media, posts, publish, render, research, settings::Settings, tools};
use anyhow::{anyhow, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone)]
pub struct Ctx {
    pub db: Arc<Db>,
    pub data_dir: PathBuf,
}

/// Jobs that load the GPU/CPU or the network hard; held back while live if the user asks.
fn is_heavy(kind: &str) -> bool {
    matches!(kind, "transcribe" | "analyze" | "render" | "thumbnails" | "publish" | "install_whisper")
}

pub async fn worker_loop(ctx: Ctx, kick: Arc<Mutex<KickState>>) {
    let db = ctx.db.clone();
    loop {
        let live = kick.lock().unwrap().live;
        let hold_heavy = live && Settings::load(&db).pause_while_live;
        match db.claim_next() {
            Ok(Some(job)) if hold_heavy && is_heavy(&job.kind) => {
                let _ = db.conn().execute(
                    "UPDATE jobs SET status='queued', attempts=attempts-1, message='Paused while you are live' WHERE id=?1",
                    [job.id],
                );
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
            Ok(Some(job)) if is_heavy(&job.kind) && low_disk(&ctx) => {
                let _ = db.conn().execute(
                    "UPDATE jobs SET status='queued', attempts=attempts-1, message='Waiting: less than 2 GB free disk space', run_after=strftime('%Y-%m-%dT%H:%M:%SZ','now','+5 minutes') WHERE id=?1",
                    [job.id],
                );
                db.log("warn", "disk", "Processing paused: less than 2 GB free. Free up space or change the output folder in Settings.");
            }
            Ok(Some(job)) => match run(&ctx, &job).await {
                Ok(msg) => {
                    let _ = db.job_done(job.id, &msg);
                }
                Err(e) => {
                    let err = format!("{e:#}");
                    db.log("error", &job.kind, &format!("Job #{} failed (attempt {}): {err}", job.id, job.attempts));
                    let _ = db.job_failed(&job, &err);
                    on_failure(&db, &job, &err);
                }
            },
            Ok(None) => tokio::time::sleep(Duration::from_secs(2)).await,
            Err(e) => {
                db.log("error", "queue", &format!("Queue error: {e:#}"));
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        }
    }
}

fn low_disk(ctx: &Ctx) -> bool {
    let out = Settings::load(&ctx.db).output_dir.unwrap_or_else(|| ctx.data_dir.to_string_lossy().to_string());
    [out, ctx.data_dir.to_string_lossy().to_string()].iter().any(|p| media::disk_space(p).is_some_and(|(free, _)| free < 2_000_000_000))
}

fn payload_id(job: &Job, key: &str) -> Result<i64> {
    serde_json::from_str::<Value>(&job.payload)?[key].as_i64().ok_or_else(|| anyhow!("job has no {key}"))
}

fn on_failure(db: &Db, job: &Job, err: &str) {
    let last = job.attempts >= job.max_attempts;
    match job.kind.as_str() {
        "render" => {
            if let Ok(id) = payload_id(job, "clip") {
                let _ = db.conn().execute("UPDATE clips SET render_status=?2, error=?3 WHERE id=?1", rusqlite::params![id, if last { "error" } else { "queued" }, err]);
            }
        }
        "publish" => {
            if let Ok(id) = payload_id(job, "post") {
                let _ = db.conn().execute("UPDATE posts SET status=?2, error=?3 WHERE id=?1", rusqlite::params![id, if last { "failed" } else { "scheduled" }, err]);
            }
        }
        _ => {
            if let (Some(sid), true) = (job.stream_id, last) {
                let _ = db.conn().execute("UPDATE streams SET status='error', error=?2 WHERE id=?1", rusqlite::params![sid, err]);
            }
        }
    }
}

async fn run(ctx: &Ctx, job: &Job) -> Result<String> {
    let db = &ctx.db;
    match job.kind.as_str() {
        "probe" => probe(ctx, job).await,
        "install_whisper" => {
            let s = Settings::load(db);
            let gpu = s.use_gpu && media::detect_toolchain().await.gpu.is_some();
            tools::install_whisper(db, job.id, &ctx.data_dir, &s.whisper_model, gpu).await
        }
        "transcribe" => transcribe(ctx, job).await,
        "analyze" => analyze(ctx, job).await,
        "render" => render_clip(ctx, job).await,
        "thumbnails" => thumbnails(ctx, job).await,
        "publish" => publish_post(ctx, job).await,
        "refresh_stats" => research::refresh_stats(db).await,
        "trend_report" => research::trend_report(db, &Settings::load(db).trend_keywords).await,
        "refresh_watchlist" => research::refresh_watchlist(db).await,
        other => Err(anyhow!("no handler for job kind '{other}'")),
    }
}

fn stream_file(db: &Db, sid: i64) -> Result<(String, f64, String)> {
    let s = db.get_stream(sid)?.ok_or_else(|| anyhow!("stream #{sid} no longer exists"))?;
    let path = s.file_path.ok_or_else(|| anyhow!("stream #{sid} has no recording file"))?;
    if !Path::new(&path).is_file() {
        return Err(anyhow!("recording file is missing: {path}"));
    }
    Ok((path, s.duration_s.unwrap_or(0.0), s.title))
}

pub fn work_dir(ctx: &Ctx, sid: i64) -> PathBuf {
    ctx.data_dir.join("work").join(sid.to_string())
}

pub fn output_dir(ctx: &Ctx, sid: i64) -> PathBuf {
    let base = Settings::load(&ctx.db).output_dir.map(PathBuf::from).unwrap_or_else(|| ctx.data_dir.join("output"));
    base.join(sid.to_string())
}

pub fn load_words(db: &Db, sid: i64) -> Result<Vec<analysis::Word>> {
    let raw: String = db
        .conn()
        .query_row("SELECT words FROM transcripts WHERE stream_id=?1", [sid], |r| r.get(0))
        .map_err(|_| anyhow!("this recording hasn't been transcribed yet"))?;
    let tuples: Vec<(f64, f64, String)> = serde_json::from_str(&raw)?;
    Ok(tuples.into_iter().map(|(s, e, t)| analysis::Word { s, e, t }).collect())
}

fn set_stream_status(db: &Db, sid: i64, status: &str) -> Result<()> {
    db.conn().execute("UPDATE streams SET status=?2, error=NULL WHERE id=?1", rusqlite::params![sid, status])?;
    Ok(())
}

// ---------- probe ----------

async fn probe(ctx: &Ctx, job: &Job) -> Result<String> {
    let db = &ctx.db;
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
    let s = Settings::load(db);
    let engine = tools::status(&ctx.data_dir, &s.whisper_model);
    if engine.whisper_installed && engine.model_installed {
        db.enqueue("transcribe", Some(sid), "{}")?;
        Ok(format!("{len} recording; transcription queued"))
    } else {
        db.log("warn", "probe", "Transcription engine isn't installed yet: Settings → AI processing → Install");
        Ok(format!("{len} recording ready; install the transcription engine to find clips"))
    }
}

fn hms(secs: f64) -> String {
    let s = secs.round() as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

// ---------- transcribe ----------

async fn transcribe(ctx: &Ctx, job: &Job) -> Result<String> {
    let db = &ctx.db;
    let sid = job.stream_id.ok_or_else(|| anyhow!("no stream"))?;
    let (video, _, title) = stream_file(db, sid)?;
    let model_name = Settings::load(db).whisper_model;
    let exe = tools::whisper_exe(&ctx.data_dir);
    let model = tools::model_path(&ctx.data_dir, &model_name);
    if !exe.is_file() || !model.is_file() {
        return Err(anyhow!("transcription engine not installed (Settings → AI processing)"));
    }
    let work = work_dir(ctx, sid);
    std::fs::create_dir_all(&work)?;
    let wav = work.join("audio.wav");
    db.job_progress(job.id, 0.02, "Extracting audio");
    analysis::extract_audio(&video, &wav).await?;
    db.job_progress(job.id, 0.08, "Transcribing speech");
    let jid = job.id;
    let (lang, words) = analysis::run_whisper(&exe, &model, &wav, &work.join("transcript"), |p| db.job_progress(jid, 0.08 + p * 0.9, "Transcribing speech")).await?;
    let _ = std::fs::remove_file(&wav);
    let tuples: Vec<(f64, f64, &str)> = words.iter().map(|w| (w.s, w.e, w.t.as_str())).collect();
    db.conn().execute(
        "INSERT INTO transcripts(stream_id, language, words) VALUES(?1,?2,?3)
         ON CONFLICT(stream_id) DO UPDATE SET language=excluded.language, words=excluded.words, created_at=strftime('%Y-%m-%dT%H:%M:%SZ','now')",
        rusqlite::params![sid, lang, serde_json::to_string(&tuples)?],
    )?;
    set_stream_status(db, sid, "transcribed")?;
    db.enqueue("analyze", Some(sid), "{}")?;
    db.log("info", "transcribe", &format!("{title}: {} words ({})", words.len(), lang.as_deref().unwrap_or("?")));
    Ok(format!("{} words transcribed", words.len()))
}

// ---------- analyze ----------

async fn analyze(ctx: &Ctx, job: &Job) -> Result<String> {
    let db = &ctx.db;
    let sid = job.stream_id.ok_or_else(|| anyhow!("no stream"))?;
    let (video, duration, title) = stream_file(db, sid)?;
    let words = load_words(db, sid)?;
    db.job_progress(job.id, 0.05, "Measuring loudness");
    let rms = analysis::loudness_per_second(&video).await?;
    db.job_progress(job.id, 0.35, "Detecting scene changes");
    let scenes = analysis::scene_changes(&video).await.unwrap_or_default();
    db.job_progress(job.id, 0.7, "Scoring moments");
    let sec = analysis::features(duration, &rms, &words, &scenes);
    let exc = analysis::excitement(&sec, &analysis::Weights::default());
    let mut cands = analysis::candidates(&sec, &exc, &words, duration, 12);
    if cands.is_empty() {
        set_stream_status(db, sid, "analyzed")?;
        return Ok(if duration < analysis::MIN_CLIP { "Recording is shorter than 60 s; no clips".into() } else { "No clip candidates found".into() });
    }

    // Claude re-scores from the transcript when a key is set; heuristic scores stay otherwise.
    let mut llm_scores = None;
    if llm::available() {
        db.job_progress(job.id, 0.8, "Asking Claude to rate the moments");
        let inputs: Vec<(f64, String)> = cands.iter().map(|c| (c.end - c.start, c.transcript.clone())).collect();
        match llm::score_candidates(&inputs, &research::latest_trend_notes(db)).await {
            Ok(s) => llm_scores = Some(s),
            Err(e) => db.log("warn", "analyze", &format!("Claude scoring skipped, using heuristic scores: {e:#}")),
        }
    }
    let perf = research::category_performance(db).unwrap_or_default();

    let kept = {
        let conn = db.conn();
        conn.execute("DELETE FROM clips WHERE stream_id=?1 AND status='suggested'", [sid])?;
        let mut kept = 0;
        for (i, c) in cands.iter_mut().enumerate() {
            let (mut sc, mut cat, mut ttl, mut hook, mut reason, mut status, mut by) =
                (c.scores.clone(), c.category.clone(), c.title.clone(), String::new(), String::new(), "suggested", "heuristic");
            if let Some(l) = llm_scores.as_ref().and_then(|v| v.get(i)) {
                let [h, en, em, cx, rp, sh, tr] = l.scores;
                sc = analysis::Scores { hook: h, entertainment: en, emotion: em, context: cx, replay: rp, shareability: sh, trend: tr, total: 0.0 };
                sc.total = (h * 0.2 + en * 0.2 + em * 0.15 + cx * 0.15 + rp * 0.1 + sh * 0.1 + tr * 0.1).round();
                cat = l.category.clone();
                if !l.title.is_empty() { ttl = l.title.clone(); }
                hook = l.hook.clone();
                reason = l.reason.clone();
                by = "claude";
                if !l.keep { status = "rejected"; }
            }
            sc.total = (sc.total * research::category_boost(&perf, &cat)).min(100.0).round();
            if status == "suggested" { kept += 1; }
            conn.execute(
                "INSERT INTO clips(stream_id,start_s,end_s,score,scores,category,title,hook,reason,transcript,scored_by,status)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
                rusqlite::params![sid, c.start, c.end, sc.total, serde_json::to_string(&sc)?, cat, ttl, hook, reason, c.transcript, by, status],
            )?;
        }
        conn.execute("UPDATE streams SET status='analyzed', error=NULL WHERE id=?1", [sid])?;
    kept
    };

    let s = Settings::load(db);
    db.enqueue("thumbnails", Some(sid), "{}")?;
    let top: Vec<i64> = {
        let conn = db.conn();
        let mut st = conn.prepare("SELECT id FROM clips WHERE stream_id=?1 AND status='suggested' ORDER BY score DESC LIMIT ?2")?;
        let rows = st.query_map(rusqlite::params![sid, s.auto_render_top], |r| r.get(0))?;
        rows.collect::<Result<_, _>>()?
    };
    for id in &top {
        posts::queue_render(db, *id)?;
    }
    if s.auto_publish {
        for id in top.iter().take(s.posts_per_day as usize) {
            posts::approve_clip(db, *id).await?;
            posts::auto_schedule_clip(db, *id).await?;
        }
    }
    db.log("info", "analyze", &format!("{title}: {kept} clip(s) suggested ({})", if llm_scores.is_some() { "rated by Claude" } else { "heuristic scores" }));
    Ok(format!("{kept} clips suggested, {} rendering", top.len()))
}

// ---------- render ----------

async fn render_clip(ctx: &Ctx, job: &Job) -> Result<String> {
    let db = &ctx.db;
    let id = payload_id(job, "clip")?;
    let (sid, start, end, hook, title): (i64, f64, f64, Option<String>, Option<String>) = db.conn().query_row(
        "SELECT stream_id,start_s,end_s,hook,title FROM clips WHERE id=?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    )?;
    let (video, _, _) = stream_file(db, sid)?;
    let words = load_words(db, sid).unwrap_or_default();
    let s = Settings::load(db);
    let nvenc = s.use_gpu && media::detect_toolchain().await.nvenc;
    let layout = render::Layout { kind: s.clip_layout.clone(), facecam: s.facecam_rect };
    let out = output_dir(ctx, sid).join(format!("clip_{id}.mp4"));
    let work = work_dir(ctx, sid).join(format!("clip_{id}"));
    let hook_text = hook.filter(|h| !h.is_empty()).or_else(|| title.as_ref().map(|t| render::headline(t)));
    let jid = job.id;
    render::render_clip(
        render::ClipJob { video: &video, start, end, words: &words, hook: hook_text.as_deref(), layout, subtitles: s.subtitles, nvenc, work: work.clone(), out: out.clone() },
        |p| db.job_progress(jid, p * 0.92, "Rendering clip"),
    )
    .await?;
    db.job_progress(job.id, 0.95, "Making cover");
    let cover = output_dir(ctx, sid).join(format!("clip_{id}_cover.jpg"));
    let layout = render::Layout { kind: s.clip_layout.clone(), facecam: s.facecam_rect };
    let cover_text = hook_text.clone().unwrap_or_default();
    let cover_ok = render::render_cover(&video, (start + 3.0).min(end), &cover_text, &layout, &work, &cover).await.is_ok();
    let _ = std::fs::remove_dir_all(&work);
    db.conn().execute(
        "UPDATE clips SET render_status='done', render_path=?2, cover_path=?3, error=NULL WHERE id=?1",
        rusqlite::params![id, out.to_string_lossy(), cover_ok.then(|| cover.to_string_lossy().to_string())],
    )?;
    Ok(format!("Clip rendered ({:.0} s, {})", end - start, if nvenc { "NVENC" } else { "CPU" }))
}

// ---------- thumbnails ----------

async fn thumbnails(ctx: &Ctx, job: &Job) -> Result<String> {
    let db = &ctx.db;
    let sid = job.stream_id.ok_or_else(|| anyhow!("no stream"))?;
    let (video, _, stream_title) = stream_file(db, sid)?;
    let picks: Vec<(f64, f64, Option<String>)> = {
        let conn = db.conn();
        let mut st = conn.prepare("SELECT start_s,end_s,title FROM clips WHERE stream_id=?1 AND status!='rejected' ORDER BY score DESC LIMIT 3")?;
        let rows = st.query_map([sid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect::<Result<_, _>>()?
    };
    if picks.is_empty() {
        return Ok("No moments to build thumbnails from".into());
    }
    db.conn().execute("DELETE FROM thumbnails WHERE stream_id=?1", [sid])?;
    let dir = output_dir(ctx, sid);
    let work = work_dir(ctx, sid).join("thumbs");
    for (rank, (start, end, title)) in picks.iter().enumerate() {
        db.job_progress(job.id, rank as f64 / picks.len() as f64, "Composing thumbnails");
        let at = (start + 35.0).min(end - 1.0); // the peak sits ~35 s after the clip start
        let text = render::headline(title.as_deref().unwrap_or(&stream_title));
        let out = dir.join(format!("thumb_{}.png", rank + 1));
        render::render_thumbnail(&video, at, &text, rank, &work, &out).await?;
        db.conn().execute(
            "INSERT INTO thumbnails(stream_id,path,rank,concept,selected) VALUES(?1,?2,?3,?4,?5)",
            rusqlite::params![sid, out.to_string_lossy(), rank as i64 + 1, render::thumbnail_concepts()[rank % 3].0, (rank == 0) as i64],
        )?;
    }
    let _ = std::fs::remove_dir_all(&work);
    Ok(format!("{} thumbnail concepts", picks.len()))
}

// ---------- publish ----------

async fn publish_post(ctx: &Ctx, job: &Job) -> Result<String> {
    let db = &ctx.db;
    let id = payload_id(job, "post")?;
    let p = posts::get_post(db, id)?;
    if p.status == "published" {
        return Ok("Already published".into());
    }
    let (file, duration, thumb) = posts::media_for(db, &p)?;
    db.conn().execute("UPDATE posts SET status='uploading', progress=0, error=NULL WHERE id=?1", [id])?;
    let tags: Vec<String> = serde_json::from_str(&p.tags).unwrap_or_default();
    let up = publish::Upload {
        file: &file,
        title: &p.title,
        description: &p.description,
        tags: &tags,
        privacy: &p.privacy,
        publish_at: None,
        thumbnail: thumb.as_deref(),
        duration_s: duration,
    };
    let jid = job.id;
    let prog = |x: f64| {
        db.job_progress(jid, x, "Uploading");
        let _ = db.conn().execute("UPDATE posts SET progress=?2 WHERE id=?1", rusqlite::params![id, x]);
    };
    let res = match p.platform.as_str() {
        "youtube" => publish::youtube(&up, prog).await?,
        "tiktok" => publish::tiktok(&up, prog).await?,
        other => return Err(anyhow!("publishing to {other} isn't available yet")),
    };
    db.conn().execute(
        "UPDATE posts SET status='published', progress=1, remote_id=?2, url=?3, error=?4, published_at=strftime('%Y-%m-%dT%H:%M:%SZ','now') WHERE id=?1",
        rusqlite::params![id, res.remote_id, res.url, res.note],
    )?;
    db.log("info", "publish", &format!("Published \"{}\" to {}", p.title, p.platform));
    Ok(format!("Published: {}", res.url))
}

// ---------- scheduled maintenance ----------

/// Every 10 minutes: stats every 6 h, trend report and watchlist once a day.
pub async fn scheduler_loop(db: Arc<Db>) {
    loop {
        let due = |kind: &str, hours: i64| -> bool {
            let last: Option<String> = db.conn().query_row("SELECT MAX(created_at) FROM jobs WHERE kind=?1", [kind], |r| r.get(0)).ok().flatten();
            last.and_then(|l| chrono::DateTime::parse_from_rfc3339(&l).ok()).map_or(true, |t| chrono::Utc::now().signed_duration_since(t).num_hours() >= hours)
        };
        let count = |sql: &str| -> i64 { db.conn().query_row(sql, [], |r| r.get(0)).unwrap_or(0) };
        let connected = |p: &str| -> bool { db.conn().query_row("SELECT COUNT(*) FROM accounts WHERE platform=?1", [p], |r| r.get::<_, i64>(0)).unwrap_or(0) > 0 };
        if connected("youtube") {
            if count("SELECT COUNT(*) FROM posts WHERE platform='youtube' AND status='published'") > 0 && due("refresh_stats", 6) {
                let _ = db.enqueue("refresh_stats", None, "{}");
            }
            if !Settings::load(&db).trend_keywords.is_empty() && due("trend_report", 22) {
                let _ = db.enqueue("trend_report", None, "{}");
            }
            if count("SELECT COUNT(*) FROM watchlist") > 0 && due("refresh_watchlist", 22) {
                let _ = db.enqueue("refresh_watchlist", None, "{}");
            }
        }
        tokio::time::sleep(Duration::from_secs(600)).await;
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn hms_formats() {
        assert_eq!(super::hms(12.02), "0:12");
        assert_eq!(super::hms(7265.0), "2:01:05");
    }
}
