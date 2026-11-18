//! Job worker: claims one job at a time from the queue and runs its handler.
//! One at a time on purpose, so processing never takes over the PC while gaming.
//!
//! Recording flow: (find + download the Kick VOD, if no local recording) → probe → transcribe
//! → analyze → (render top clips, thumbnails, upload the full stream if set) → you approve
//! clips → publish. Scheduled jobs: stats, trend report, watchlist.

use crate::{analysis, censor, db::Db, jobs::{now_str, Job}, kick::{self, KickState}, llm, media, nudity, posts, publish, render, research, settings::Settings, tools};
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
    matches!(kind, "download_vod" | "transcribe" | "analyze" | "render" | "thumbnails" | "publish" | "install_whisper" | "install_speakers")
}

pub async fn worker_loop(ctx: Ctx, kick: Arc<Mutex<KickState>>) {
    let db = ctx.db.clone();
    loop {
        let live = kick.lock().unwrap().live;
        let hold_heavy = live && Settings::load(&db).pause_while_live;
        match db.claim_next() {
            Ok(Some(job)) if hold_heavy && is_heavy(&job.kind) => {
                let _ = db.conn().execute(
                    // Held back a minute, so lighter jobs behind it in the queue still run.
                    "UPDATE jobs SET status='queued', attempts=attempts-1, message='Paused while you are live', run_after=strftime('%Y-%m-%dT%H:%M:%SZ','now','+1 minutes') WHERE id=?1 AND status='running'",
                    [job.id],
                );
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
            Ok(Some(job)) if is_heavy(&job.kind) && low_disk(&ctx) => {
                let _ = db.conn().execute(
                    "UPDATE jobs SET status='queued', attempts=attempts-1, message='Waiting: less than 2 GB free disk space', run_after=strftime('%Y-%m-%dT%H:%M:%SZ','now','+5 minutes') WHERE id=?1 AND status='running'",
                    [job.id],
                );
                db.log("warn", "disk", "Processing paused: less than 2 GB free. Free up space or change the output folder in Settings.");
            }
            Ok(Some(job)) => match crate::jobs::with_cancel(job.id, run(&ctx, &job)).await {
                Ok(msg) => {
                    let _ = db.job_done(job.id, &msg);
                }
                Err(e) if was_cancelled(&db, job.id, &e) => {
                    db.log("info", &job.kind, &format!("Job #{} cancelled", job.id));
                    on_cancel(&db, &job);
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

/// A job the user cancelled ends as cancelled, not as a failure to retry.
fn was_cancelled(db: &Db, id: i64, e: &anyhow::Error) -> bool {
    e.chain().any(|c| c.is::<crate::jobs::Cancelled>())
        || db.conn().query_row("SELECT status FROM jobs WHERE id=?1", [id], |r| r.get::<_, String>(0)).is_ok_and(|s| s == "cancelled")
}

fn low_disk(ctx: &Ctx) -> bool {
    let out = Settings::load(&ctx.db).output_dir.unwrap_or_else(|| ctx.data_dir.to_string_lossy().to_string());
    [out, ctx.data_dir.to_string_lossy().to_string()].iter().any(|p| media::disk_space(p).is_some_and(|(free, _)| free < 2_000_000_000))
}

fn payload_id(job: &Job, key: &str) -> Result<i64> {
    serde_json::from_str::<Value>(&job.payload)?[key].as_i64().ok_or_else(|| anyhow!("job has no {key}"))
}

/// A cancelled render or upload must not leave its clip or post stuck in a working state.
fn on_cancel(db: &Db, job: &Job) {
    match job.kind.as_str() {
        "render" => {
            if let Ok(id) = payload_id(job, "clip") {
                let _ = db.conn().execute("UPDATE clips SET render_status='error', error='Cancelled' WHERE id=?1 AND render_status!='done'", [id]);
            }
        }
        "publish" => {
            if let Ok(id) = payload_id(job, "post") {
                let _ = db.conn().execute("UPDATE posts SET status='failed', error='Cancelled' WHERE id=?1 AND status='uploading'", [id]);
            }
        }
        _ => {}
    }
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
        "install_speakers" => tools::install_speakers(db, job.id, &ctx.data_dir).await,
        "find_vod" => {
            let sid = job.stream_id.ok_or_else(|| anyhow!("no stream"))?;
            let attempt = serde_json::from_str::<Value>(&job.payload)?["try"].as_i64().unwrap_or(1);
            let slug = Settings::load(db).kick_channel_slug.ok_or_else(|| anyhow!("no Kick channel set"))?;
            kick::find_vod(db, sid, attempt, &slug).await
        }
        "download_vod" => download_vod(ctx, job).await,
        "transcribe" => transcribe(ctx, job).await,
        "analyze" => analyze(ctx, job).await,
        "render" => render_clip(ctx, job).await,
        "thumbnails" => thumbnails(ctx, job).await,
        "publish" => publish_post(ctx, job).await,
        "upload_captions" => upload_captions(ctx, job).await,
        "update_video" => {
            let p = posts::get_post(db, payload_id(job, "post")?)?;
            let id = p.remote_id.ok_or_else(|| anyhow!("this post has no YouTube video"))?;
            let tags: Vec<String> = serde_json::from_str(&p.tags).unwrap_or_default();
            // The thumbnail first: it still applies if YouTube refuses the privacy change.
            let mut thumb_note = String::new();
            if let Some(sid) = p.stream_id {
                let thumb: Option<String> = db.conn().query_row("SELECT path FROM thumbnails WHERE stream_id=?1 AND selected=1", [sid], |r| r.get(0)).ok();
                if let Some(t) = thumb.filter(|t| Path::new(t).is_file()) {
                    if let Err(e) = publish::youtube_thumbnail(&id, Path::new(&t)).await {
                        thumb_note = format!("; {e:#}");
                    }
                }
            }
            let v = publish::youtube_update(&id, &p.title, &p.description, &tags, &p.privacy).await?;
            Ok(format!("Updated on YouTube ({}){thumb_note}", v.privacy))
        }
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
    // Censored words are stored masked, so the mask is the record of what's bleeped.
    let words = tuples.into_iter().map(|(s, e, t)| analysis::Word { s, e, b: censor::is_masked(&t), t, sp: 0 }).collect();
    Ok(analysis::drop_sound_tags(words))
}

/// A clip's editable words, or its slice of the transcript for clips made before they existed.
pub fn clip_words(db: &Db, id: i64, sid: i64, start: f64, end: f64) -> Vec<analysis::Word> {
    let own: Option<String> = db.conn().query_row("SELECT words FROM clips WHERE id=?1", [id], |r| r.get(0)).ok().flatten();
    own.and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_else(|| analysis::slice_words(&load_words(db, sid).unwrap_or_default(), start, end))
}

/// Saves a clip's words and keeps its transcript text in step.
pub fn save_clip_words(db: &Db, id: i64, words: &[analysis::Word]) -> Result<()> {
    db.conn().execute("UPDATE clips SET words=?2, transcript=?3 WHERE id=?1", rusqlite::params![id, serde_json::to_string(words)?, analysis::words_text(words)])?;
    Ok(())
}

fn ranges(words: &[analysis::Word]) -> Vec<(f64, f64)> {
    words.iter().filter(|w| w.b).map(|w| censor::range(w.s, w.e)).collect()
}

/// Everything to bleep in a full stream: the censor's hits plus words bleeped by hand in clips.
fn stream_censors(db: &Db, sid: i64) -> Result<Vec<(f64, f64)>> {
    let mut r = ranges(&load_words(db, sid)?);
    let clips: Vec<String> = {
        let conn = db.conn();
        let mut st = conn.prepare("SELECT words FROM clips WHERE stream_id=?1 AND words IS NOT NULL")?;
        let rows = st.query_map([sid], |r| r.get(0))?;
        rows.collect::<Result<_, _>>()?
    };
    for c in clips {
        if let Ok(w) = serde_json::from_str::<Vec<analysis::Word>>(&c) {
            r.extend(ranges(&w));
        }
    }
    Ok(censor::merge(r))
}

/// Transcripts from before chunked transcription are full of repeats and aren't censored;
/// engine 2 ones have word times up to 14 s early (whisper's `--vad` timeline mapping).
/// Queues them once at startup; heavy work still waits while you're live.
///
/// Engine 2 ones are only redone when the voice detection tool is installed: without it the
/// new transcript would have the same early times, and be redone again on every start.
pub fn redo_old_transcripts(db: &Db, data_dir: &Path) -> Result<usize> {
    let below = if analysis::speech_detector(&tools::whisper_exe(data_dir)).is_file() { 3 } else { 2 };
    let ids: Vec<i64> = {
        let conn = db.conn();
        let mut st = conn.prepare("SELECT t.stream_id FROM transcripts t JOIN streams s ON s.id=t.stream_id WHERE t.engine<?1 AND s.file_path IS NOT NULL")?;
        let rows = st.query_map([below], |r| r.get(0))?;
        rows.collect::<Result<_, _>>()?
    };
    for id in &ids {
        db.enqueue("transcribe", Some(*id), "{}")?;
    }
    Ok(ids.len())
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
    let s = Settings::load(db);
    let exe = tools::whisper_exe(&ctx.data_dir);
    let model = tools::model_path(&ctx.data_dir, &s.whisper_model);
    if !exe.is_file() || !model.is_file() {
        return Err(anyhow!("transcription engine not installed (Settings → AI processing)"));
    }
    let vad = tools::ensure_vad(&ctx.data_dir).await?;
    let work = work_dir(ctx, sid);
    std::fs::create_dir_all(&work)?;
    let raw = work.join("audio.pcm");
    let pieces = work.join("pieces");
    let _ = std::fs::remove_dir_all(&pieces);
    db.job_progress(job.id, 0.02, "Extracting audio");
    analysis::extract_audio(&video, &raw).await?;
    let chunks = analysis::split_audio(&raw, &pieces)?;
    let _ = std::fs::remove_file(&raw);
    db.job_progress(job.id, 0.08, "Transcribing speech");
    let jid = job.id;
    let share = if s.censor_enabled { 0.4 } else { 0.9 };
    let heard = analysis::run_whisper(&exe, &model, Some(&vad), &pieces, &chunks, |p| db.job_progress(jid, 0.08 + p * share, "Transcribing speech")).await?;
    let (lang, mut words) = (heard.lang, heard.words);
    let mut hits = 0;
    if s.censor_enabled {
        hits = censor::apply(&mut words, &s.censor_words, &heard.english).len();
        db.job_progress(job.id, 0.5, "Second listen for the censor");
        match analysis::run_whisper(&exe, &model, None, &pieces, &chunks, |p| db.job_progress(jid, 0.5 + p * 0.48, "Second listen for the censor")).await {
            Ok(second) => hits += censor::add_missed(&mut words, &second.words, &s.censor_words, &second.english),
            Err(e) => db.log("warn", "transcribe", &format!("Second censor listen failed, using the first: {e:#}")),
        }
    }
    let _ = std::fs::remove_dir_all(&pieces);
    let tuples: Vec<(f64, f64, &str)> = words.iter().map(|w| (w.s, w.e, w.t.as_str())).collect();
    db.conn().execute(
        "INSERT INTO transcripts(stream_id, language, words, engine) VALUES(?1,?2,?3,?4)
         ON CONFLICT(stream_id) DO UPDATE SET language=excluded.language, words=excluded.words, engine=excluded.engine, created_at=strftime('%Y-%m-%dT%H:%M:%SZ','now')",
        // Engine 3 means the times were made with the voice detection tool; without it they are engine 2's.
        rusqlite::params![sid, lang, serde_json::to_string(&tuples)?, if analysis::speech_detector(&exe).is_file() { 3 } else { 2 }],
    )?;
    // Anything built from the old transcript is out of date.
    db.conn().execute("UPDATE streams SET censored_path=NULL, meta=NULL WHERE id=?1", [sid])?;
    let approved: Vec<(i64, f64, f64)> = {
        let conn = db.conn();
        let mut st = conn.prepare("SELECT id,start_s,end_s FROM clips WHERE stream_id=?1 AND status='approved'")?;
        let rows = st.query_map([sid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect::<Result<_, _>>()?
    };
    for (id, start, end) in approved {
        let old: Vec<analysis::Word> = db.conn().query_row("SELECT words FROM clips WHERE id=?1", [id], |r| r.get::<_, Option<String>>(0)).ok().flatten().and_then(|w| serde_json::from_str(&w).ok()).unwrap_or_default();
        save_clip_words(db, id, &analysis::keep_bleeps(analysis::slice_words(&words, start, end), &old))?;
        posts::queue_render(db, id)?;
    }
    set_stream_status(db, sid, "transcribed")?;
    db.enqueue("analyze", Some(sid), "{}")?;
    db.log("info", "transcribe", &format!("{title}: {} words ({}), {hits} censored", words.len(), lang.as_deref().unwrap_or("?")));
    Ok(format!("{} words transcribed, {hits} censored", words.len()))
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
    let cuts = analysis::dead_spans(&sec, &words, duration);
    db.conn().execute("UPDATE streams SET cuts=?2 WHERE id=?1", rusqlite::params![sid, serde_json::to_string(&cuts)?])?;
    let exc = analysis::excitement(&sec, &analysis::Weights::default());
    let mut cands = analysis::candidates(&sec, &exc, &words, duration, 12);
    if cands.is_empty() {
        set_stream_status(db, sid, "analyzed")?;
        upload_if_set(db, sid).await;
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
        // On a second pass over a stream, a moment that is already a clip (approved, posted or
        // rejected) is not suggested again: it would be rendered and could be posted twice.
        let taken: Vec<(f64, f64)> = {
            let mut st = conn.prepare("SELECT start_s,end_s FROM clips WHERE stream_id=?1")?;
            let rows = st.query_map([sid], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<Result<_, _>>()?
        };
        let mut kept = 0;
        for (i, c) in cands.iter_mut().enumerate() {
            if taken.iter().any(|(a, b)| c.end.min(*b) - c.start.max(*a) > (c.end - c.start) / 2.0) {
                continue;
            }
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
            let clip_words = serde_json::to_string(&analysis::slice_words(&words, c.start, c.end))?;
            conn.execute(
                "INSERT INTO clips(stream_id,start_s,end_s,score,scores,category,title,hook,reason,transcript,scored_by,status,words)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
                rusqlite::params![sid, c.start, c.end, sc.total, serde_json::to_string(&sc)?, cat, ttl, hook, reason, c.transcript, by, status, clip_words],
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

/// Width / height of a recording, 16:9 if it can't be read.
async fn aspect(video: &str) -> f64 {
    match media::probe(video).await {
        Ok(media::ProbeInfo { width: Some(w), height: Some(h), .. }) if w > 0 && h > 0 => w as f64 / h as f64,
        _ => 16.0 / 9.0,
    }
}

/// On-screen nudity in `dur` seconds of the recording from `start` (to the end when `dur` is 0),
/// on that window's own timeline. Empty when the nudity censor is off.
async fn nudity_regions(ctx: &Ctx, s: &Settings, video: &str, start: f64, dur: f64, progress: impl FnMut(f64)) -> Result<Vec<nudity::Region>> {
    if s.nudity_censor == "off" {
        return Ok(Vec::new());
    }
    let model = nudity::ensure_model(&ctx.data_dir).await?;
    let hits = nudity::scan(&model, video, start, dur, aspect(video).await, s.nudity_threshold as f32, progress).await?;
    Ok(nudity::regions(&hits))
}

/// Nudity regions of a whole stream, saved next to its output so a retry doesn't scan again.
async fn stream_nudity(ctx: &Ctx, job: &Job, sid: i64, s: &Settings, video: &str, duration: f64) -> Result<Vec<nudity::Region>> {
    if s.nudity_censor == "off" {
        return Ok(Vec::new());
    }
    let cache = output_dir(ctx, sid).join(nudity::cache_name(video, s.nudity_threshold));
    if let Some(r) = std::fs::read_to_string(&cache).ok().and_then(|j| serde_json::from_str(&j).ok()) {
        return Ok(r);
    }
    let db = &ctx.db;
    let jid = job.id;
    let r = nudity_regions(ctx, s, video, 0.0, duration, |p| db.job_progress(jid, p, "Checking the picture for nudity")).await?;
    std::fs::create_dir_all(cache.parent().unwrap())?;
    std::fs::write(&cache, serde_json::to_string(&r)?)?;
    if !r.is_empty() {
        db.log("info", "publish", &format!("Pixelating nudity in {} spot(s) of the full stream", r.len()));
    }
    Ok(r)
}

async fn render_clip(ctx: &Ctx, job: &Job) -> Result<String> {
    let db = &ctx.db;
    let id = payload_id(job, "clip")?;
    let (sid, start, end, hook, title): (i64, f64, f64, Option<String>, Option<String>) = db.conn().query_row(
        "SELECT stream_id,start_s,end_s,hook,title FROM clips WHERE id=?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    )?;
    let (video, _, _) = stream_file(db, sid)?;
    let mut words = clip_words(db, id, sid, start, end);
    let s = Settings::load(db);
    let nvenc = s.use_gpu && media::detect_toolchain().await.nvenc;
    let layout = render::Layout { kind: s.clip_layout.clone(), facecam: s.facecam_rect };
    let out = output_dir(ctx, sid).join(format!("clip_{id}.mp4"));
    let work = work_dir(ctx, sid).join(format!("clip_{id}"));
    let hook_text = hook.filter(|h| !h.is_empty()).or_else(|| title.as_ref().map(|t| render::headline(t)));
    let jid = job.id;
    std::fs::create_dir_all(&work)?;
    if let Some(tool) = tools::speaker_tool(&ctx.data_dir) {
        db.job_progress(job.id, 0.02, "Telling speakers apart");
        match analysis::diarize(&tool, &video, start, end - start, &work).await {
            Ok(turns) => {
                analysis::assign_speakers(&mut words, &turns, start);
                save_clip_words(db, id, &words)?;
            }
            Err(e) => db.log("warn", "render", &format!("Clip #{id} rendered in one colour: {e:#}")),
        }
    }
    let censors = ranges(&words);
    let nude = if s.nudity_censor == "off" {
        Vec::new()
    } else {
        db.job_progress(job.id, 0.03, "Checking the picture for nudity");
        nudity_regions(ctx, &s, &video, start, end - start, |_| {}).await?
    };
    if !nude.is_empty() {
        db.log("info", "render", &format!("Clip #{id}: pixelating nudity in {} spot(s)", nude.len()));
    }
    render::render_clip(
        render::ClipJob { video: &video, start, end, words: &words, hook: hook_text.as_deref(), layout, subtitles: s.subtitles, nvenc, work: work.clone(), out: out.clone(), censors: &censors, censor_style: &s.censor_style, nudity: &nude, nudity_style: &s.nudity_censor },
        |p| db.job_progress(jid, p * 0.92, "Rendering clip"),
    )
    .await?;
    db.job_progress(job.id, 0.95, "Making cover");
    let cover = output_dir(ctx, sid).join(format!("clip_{id}_cover.jpg"));
    let layout = render::Layout { kind: s.clip_layout.clone(), facecam: s.facecam_rect };
    let cover_text = hook_text.clone().unwrap_or_default();
    // The cover frame avoids nudity when the clip has a clean moment, and is pixelated when it doesn't.
    let clean = |t: f64| !nude.iter().any(|r| t - start >= r.start && t - start <= r.end);
    let at = [3.0, 1.0, 6.0, 10.0, 15.0].iter().map(|o| (start + o).min(end)).find(|t| clean(*t));
    let cover_ok = render::render_cover(&video, at.unwrap_or((start + 3.0).min(end)), &cover_text, &layout, at.is_none(), &work, &cover).await.is_ok();
    let _ = std::fs::remove_dir_all(&work);
    db.conn().execute(
        "UPDATE clips SET render_status='done', render_path=?2, cover_path=?3, error=NULL WHERE id=?1",
        rusqlite::params![id, out.to_string_lossy(), cover_ok.then(|| cover.to_string_lossy().to_string())],
    )?;
    Ok(format!("Clip rendered ({:.0} s, {})", end - start, if nvenc { "NVENC" } else { "CPU" }))
}

// ---------- thumbnails ----------

/// Average brightness below this (0-255) counts as a dark frame.
const DARK: f64 = 45.0;

async fn thumbnails(ctx: &Ctx, job: &Job) -> Result<String> {
    let db = &ctx.db;
    let sid = job.stream_id.ok_or_else(|| anyhow!("no stream"))?;
    let (video, _, stream_title) = stream_file(db, sid)?;
    let picks: Vec<(f64, f64)> = {
        let conn = db.conn();
        let mut st = conn.prepare("SELECT start_s,end_s FROM clips WHERE stream_id=?1 AND status!='rejected' ORDER BY score DESC LIMIT 3")?;
        let rows = st.query_map([sid], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<Result<_, _>>()?
    };
    if picks.is_empty() {
        upload_if_set(db, sid).await;
        return Ok("No moments to build thumbnails from".into());
    }
    let meta = posts::stream_meta(db, sid).await?;
    // The stream's headline, not a clip title: those are quotes from the transcript.
    let text = if meta.thumb_text.trim().is_empty() { render::headline(&stream_title) } else { meta.thumb_text.clone() };
    db.conn().execute("DELETE FROM thumbnails WHERE stream_id=?1", [sid])?;
    let dir = output_dir(ctx, sid);
    let work = work_dir(ctx, sid).join("thumbs");
    let s = Settings::load(db);
    let mut ai_at = None;
    for (rank, (start, end)) in picks.iter().enumerate() {
        db.job_progress(job.id, rank as f64 / picks.len() as f64, "Composing thumbnails");
        // Nudity in this moment, on its own timeline (none when the censor is off).
        let nude = nudity_regions(ctx, &s, &video, *start, end - start, |_| {}).await?;
        let (times, blur) = thumb_times(*start, *end, &nude);
        if rank == 0 && !blur {
            ai_at = times.first().copied();
        }
        let out = dir.join(format!("thumb_{}.png", rank + 1));
        // A dark frame (loading screen, fade) makes a dead thumbnail: try a little later.
        for at in times {
            render::render_thumbnail(&video, at, &text, rank, blur, &work, &out).await?;
            if render::brightness(&out).await.map_or(true, |b| b >= DARK) {
                break;
            }
        }
        db.conn().execute(
            "INSERT INTO thumbnails(stream_id,path,rank,concept,selected) VALUES(?1,?2,?3,?4,?5)",
            rusqlite::params![sid, out.to_string_lossy(), rank as i64 + 1, render::thumbnail_concepts()[rank % 3].0, (rank == 0) as i64],
        )?;
    }
    // A stream can open on a long loading screen, darker than any retry reaches. Select the
    // brightest concept rather than always the first.
    let made: Vec<(i64, String)> = {
        let conn = db.conn();
        let mut st = conn.prepare("SELECT id,path FROM thumbnails WHERE stream_id=?1")?;
        let rows = st.query_map([sid], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<Result<_, _>>()?
    };
    let mut best: Option<(i64, f64)> = None;
    for (id, path) in &made {
        let b = render::brightness(Path::new(path)).await.unwrap_or(0.0);
        if best.is_none_or(|(_, x)| b > x + 5.0) {
            best = Some((*id, b));
        }
    }
    if let Some((id, _)) = best {
        db.conn().execute("UPDATE thumbnails SET selected=(id=?1) WHERE stream_id=?2", rusqlite::params![id, sid])?;
    }
    let ai = if let (true, Some(at)) = (llm::gemini_available(), ai_at) {
        db.job_progress(job.id, 0.9, "Making the AI thumbnail");
        // Only a nudity-free frame goes to Gemini: `ai_at` is None when the moment has none.
        match ai_thumbnail(db, &video, at, &text, &meta.title, sid, &dir, &work).await {
            Ok(()) => " + AI thumbnail",
            Err(e) => {
                db.log("warn", "thumbnails", &format!("AI thumbnail skipped: {e:#}"));
                ""
            }
        }
    } else {
        ""
    };
    let _ = std::fs::remove_dir_all(&work);
    upload_if_set(db, sid).await;
    Ok(format!("{} thumbnail concepts{ai}", picks.len()))
}

/// Up to four times in a moment to take its thumbnail from, best first: ~35 s in (where the
/// peak sits), then 15 s steps for dark frames. A frame is read from the 4 s around its time,
/// so a time near nudity (`nude`, on the moment's timeline) is skipped. With no clean time
/// at all, the usual times come back with `true`: pixelate the whole picture.
fn thumb_times(start: f64, end: f64, nude: &[nudity::Region]) -> (Vec<f64>, bool) {
    let clean = |t: f64| !nude.iter().any(|r| r.start <= t - start + 2.0 && r.end >= t - start - 2.0);
    let last = (end - 1.0).max(start);
    let usual: Vec<f64> = (0..4).map(|k| (start + 35.0 + k as f64 * 15.0).min(last)).collect();
    let spread = (0..).map(|k| start + 2.0 + k as f64 * 5.0).take_while(|t| *t <= last);
    let mut times: Vec<f64> = usual.iter().copied().chain(spread).filter(|t| clean(*t)).collect();
    times.dedup();
    times.truncate(4);
    if times.is_empty() {
        return (usual, true);
    }
    (times, false)
}

/// Gemini restyles the strongest moment's frame; FFmpeg puts the headline on it. Becomes the
/// selected thumbnail.
#[allow(clippy::too_many_arguments)]
async fn ai_thumbnail(db: &Db, video: &str, at: f64, text: &str, title: &str, sid: i64, dir: &Path, work: &Path) -> Result<()> {
    let frame = work.join("ai_frame.jpg");
    render::plain_frame(video, at, &frame).await?;
    let category: Option<String> = db.conn().query_row("SELECT category FROM streams WHERE id=?1", [sid], |r| r.get(0))?;
    let png = llm::ai_thumbnail(&std::fs::read(&frame)?, category.as_deref().unwrap_or("the game"), title).await?;
    let raw = work.join("ai_raw.png");
    std::fs::write(&raw, png)?;
    let out = dir.join("thumb_ai.png");
    render::text_over_image(&raw, text, &work.join("ai_text"), &out).await?;
    let conn = db.conn();
    conn.execute("UPDATE thumbnails SET selected=0 WHERE stream_id=?1", [sid])?;
    conn.execute(
        "INSERT INTO thumbnails(stream_id,path,rank,concept,selected) VALUES(?1,?2,0,'AI image (Gemini)',1)",
        rusqlite::params![sid, out.to_string_lossy()],
    )?;
    Ok(())
}

/// A problem here shows in the log and leaves the clips and thumbnails that were just made.
async fn upload_if_set(db: &Db, sid: i64) {
    if let Err(e) = posts::auto_upload(db, sid).await {
        db.log("error", "publish", &format!("Couldn't queue the full stream upload: {e:#}"));
    }
}

// ---------- Kick VODs ----------

async fn download_vod(ctx: &Ctx, job: &Job) -> Result<String> {
    let db = &ctx.db;
    let sid = job.stream_id.ok_or_else(|| anyhow!("no stream"))?;
    let (title, started, url): (String, Option<String>, Option<String>) =
        db.conn().query_row("SELECT title, kick_started, vod_source FROM streams WHERE id=?1", [sid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    let url = url.ok_or_else(|| anyhow!("this stream has no Kick VOD"))?;
    let duration = serde_json::from_str::<Value>(&job.payload)?["duration"].as_f64().unwrap_or(0.0);
    let s = Settings::load(db);
    // A subfolder of the recording folder: the folder watcher only scans the top level.
    let base = s.recording_dir.or(s.output_dir).map(PathBuf::from).unwrap_or_else(|| ctx.data_dir.clone());
    let dir = base.join("kick-vods");
    std::fs::create_dir_all(&dir)?;
    let day: String = started.unwrap_or_else(now_str).chars().take(10).collect();
    let dest = dir.join(format!("{day} {}.mp4", kick::file_title(&title)));
    let part = dest.with_extension("part.mp4");
    let args: Vec<String> = ["-reconnect", "1", "-reconnect_streamed", "1", "-reconnect_delay_max", "30", "-i", &url, "-c", "copy", "-bsf:a", "aac_adtstoasc", "-movflags", "+faststart"]
        .iter()
        .map(|a| a.to_string())
        .chain([part.to_string_lossy().to_string()])
        .collect();
    let jid = job.id;
    if let Err(e) = render::run_ffmpeg(&dir, &args, duration, |p| db.job_progress(jid, p, "Downloading from Kick")).await {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    std::fs::rename(&part, &dest)?;
    let size = std::fs::metadata(&dest)?.len() as i64;
    db.conn().execute(
        "UPDATE streams SET file_path=?2, file_size=?3, status='queued', error=NULL WHERE id=?1",
        rusqlite::params![sid, dest.to_string_lossy(), size],
    )?;
    db.enqueue("probe", Some(sid), "{}")?;
    db.log("info", "kick", &format!("Downloaded the Kick VOD: {title}"));
    Ok(format!("Downloaded {:.1} GB", size as f64 / 1e9))
}

// ---------- publish ----------

/// Stretches you marked to cut in the Clip Picker.
pub fn manual_cuts(db: &Db, sid: i64) -> Vec<(f64, f64)> {
    db.conn()
        .query_row("SELECT manual_cuts FROM streams WHERE id=?1", [sid], |r| r.get::<_, String>(0))
        .ok()
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default()
}

/// Uploads the transcript as a subtitle track, timed to the video as uploaded.
async fn upload_captions(ctx: &Ctx, job: &Job) -> Result<String> {
    let db = &ctx.db;
    let p = posts::get_post(db, payload_id(job, "post")?)?;
    let sid = p.stream_id.ok_or_else(|| anyhow!("subtitles are for full streams"))?;
    let video_id = p.remote_id.ok_or_else(|| anyhow!("this post isn't on YouTube yet"))?;
    let words = load_words(db, sid)?;
    let lang: Option<String> = db.conn().query_row("SELECT language FROM transcripts WHERE stream_id=?1", [sid], |r| r.get(0))?;
    let saved: Option<String> = db.conn().query_row("SELECT upload_parts FROM streams WHERE id=?1", [sid], |r| r.get(0))?;
    let parts: Vec<(f64, f64)> = match saved.and_then(|j| serde_json::from_str(&j).ok()) {
        Some(p) => p,
        None => {
            // Uploaded before the kept parts were saved: work them out the same way again.
            let (video, duration, _) = stream_file(db, sid)?;
            let cuts: Vec<(f64, f64)> = db.conn().query_row("SELECT cuts FROM streams WHERE id=?1", [sid], |r| r.get::<_, Option<String>>(0))?
                .and_then(|j| serde_json::from_str(&j).ok())
                .unwrap_or_default();
            let parts = if cuts.is_empty() {
                vec![(0.0, duration)]
            } else {
                db.job_progress(job.id, 0.1, "Reading keyframes");
                render::kept_parts(duration, &cuts, &render::keyframes(&video).await?)
            };
            db.conn().execute("UPDATE streams SET upload_parts=?2 WHERE id=?1", rusqlite::params![sid, serde_json::to_string(&parts)?])?;
            parts
        }
    };
    let srt = render::subtitles_srt(&words, &parts);
    db.job_progress(job.id, 0.8, "Uploading subtitles");
    publish::youtube_captions(&video_id, lang.as_deref().unwrap_or("en"), &srt).await?;
    Ok(format!("Subtitles added ({} lines)", srt.matches(" --> ").count()))
}

/// Stretches to cut, worked out during analysis or now for streams analysed before that.
async fn dead_air(db: &Db, job: &Job, sid: i64, video: &str, duration: f64) -> Result<Vec<(f64, f64)>> {
    let saved: Option<String> = db.conn().query_row("SELECT cuts FROM streams WHERE id=?1", [sid], |r| r.get(0))?;
    if let Some(c) = saved.and_then(|j| serde_json::from_str(&j).ok()) {
        return Ok(c);
    }
    db.job_progress(job.id, 0.0, "Finding dead air");
    let words = load_words(db, sid)?;
    let rms = analysis::loudness_per_second(video).await?;
    let scenes = analysis::scene_changes(video).await.unwrap_or_default();
    let cuts = analysis::dead_spans(&analysis::features(duration, &rms, &words, &scenes), &words, duration);
    db.conn().execute("UPDATE streams SET cuts=?2 WHERE id=?1", rusqlite::params![sid, serde_json::to_string(&cuts)?])?;
    Ok(cuts)
}

/// Makes the bleeped copy of a full stream, if it needs one and doesn't have one yet. A stream
/// the censor hasn't read never goes out.
async fn censor_stream(ctx: &Ctx, job: &Job, sid: i64) -> Result<()> {
    let db = &ctx.db;
    let s = Settings::load(db);
    let engine: Option<i64> = db.conn().query_row("SELECT engine FROM transcripts WHERE stream_id=?1", [sid], |r| r.get(0)).ok();
    if s.censor_enabled && engine.unwrap_or(0) < 2 {
        return Err(anyhow!("this stream hasn't been through the censor yet; wait for transcription to finish"));
    }
    // A stream whose bleeps can't be read must not go out unbleeped.
    let hits = if s.censor_enabled { stream_censors(db, sid)? } else { stream_censors(db, sid).unwrap_or_default() };
    let have: Option<String> = db.conn().query_row("SELECT censored_path FROM streams WHERE id=?1", [sid], |r| r.get(0))?;
    let (video, duration, _) = stream_file(db, sid)?;
    let category: Option<String> = db.conn().query_row("SELECT category FROM streams WHERE id=?1", [sid], |r| r.get(0)).ok().flatten();
    let mut cuts = if s.cuts_dead_air(category.as_deref()) { dead_air(db, job, sid, &video, duration).await? } else { Vec::new() };
    cuts.extend(manual_cuts(db, sid));
    let cuts = censor::merge(cuts);
    // The copy is named after everything that shapes it, so a changed bleep, cut or setting
    // makes a new one instead of reusing a stale one.
    let out = output_dir(ctx, sid).join(format!("full_censored_{}.mp4", censored_key(&video, &hits, &cuts, &s)));
    if let Some(old) = have {
        if Path::new(&old) == out && out.is_file() {
            return Ok(());
        }
        let _ = std::fs::remove_file(&old);
        db.conn().execute("UPDATE streams SET censored_path=NULL WHERE id=?1", [sid])?;
    }
    let nude = stream_nudity(ctx, job, sid, &s, &video, duration).await?;
    let nvenc = s.use_gpu && media::detect_toolchain().await.nvenc;
    let pix = render::Pixelate { regions: &nude, style: &s.nudity_censor, nvenc };
    if hits.is_empty() && cuts.is_empty() && nude.is_empty() {
        db.conn().execute("UPDATE streams SET upload_parts=?2 WHERE id=?1", rusqlite::params![sid, serde_json::to_string(&[(0.0, duration)])?])?;
        return Ok(());
    }
    let jid = job.id;
    if cuts.is_empty() {
        render::censor_full_video(&video, &out, &hits, &s.censor_style, &pix, duration, |p| db.job_progress(jid, p, if nude.is_empty() { "Bleeping audio" } else { "Pixelating nudity" })).await?;
        db.conn().execute("UPDATE streams SET upload_parts=?2 WHERE id=?1", rusqlite::params![sid, serde_json::to_string(&[(0.0, duration)])?])?;
    } else {
        db.job_progress(job.id, 0.0, "Reading keyframes");
        let parts = render::kept_parts(duration, &cuts, &render::keyframes(&video).await?);
        let removed = duration - parts.iter().map(|(a, b)| b - a).sum::<f64>();
        render::edit_full_video(&video, &out, &parts, &hits, &s.censor_style, &pix, |p| db.job_progress(jid, p, "Cutting dead air and bleeping")).await?;
        db.conn().execute("UPDATE streams SET upload_parts=?2 WHERE id=?1", rusqlite::params![sid, serde_json::to_string(&parts)?])?;
        db.log("info", "publish", &format!("Cut {} of dead air from the full stream", hms(removed)));
    }
    db.conn().execute("UPDATE streams SET censored_path=?2 WHERE id=?1", rusqlite::params![sid, out.to_string_lossy()])?;
    Ok(())
}

/// Short key of the inputs of a stream's censored copy: the source file (path, size, modified
/// time), the bleeps, the cuts and the settings that change the picture or sound.
fn censored_key(video: &str, hits: &[(f64, f64)], cuts: &[(f64, f64)], s: &Settings) -> String {
    let meta = std::fs::metadata(video).ok();
    let size = meta.as_ref().map_or(0, |m| m.len());
    let mtime = meta.and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    let thr = if s.nudity_censor == "off" { String::new() } else { format!("{:.2}", s.nudity_threshold) };
    media::short_hash(&format!("{video}|{size}|{mtime}|{hits:?}|{cuts:?}|{}|{}|{thr}", s.censor_style, s.nudity_censor))
}

async fn publish_post(ctx: &Ctx, job: &Job) -> Result<String> {
    let db = &ctx.db;
    let id = payload_id(job, "post")?;
    let p = posts::get_post(db, id)?;
    if p.status == "published" {
        return Ok("Already published".into());
    }
    if let Some(sid) = p.stream_id {
        censor_stream(ctx, job, sid).await?;
    }
    let (file, duration, thumb) = posts::media_for(db, &p)?;
    // Refreshed here, with the database, so an expired sign-in shows on the Accounts page.
    if let Ok(pr) = crate::oauth::provider(&p.platform) {
        crate::oauth::access_token_db(db, pr).await?;
    }
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
    if let Some(sid) = p.stream_id {
        // The bleeped copy is as big as the stream and has done its job.
        let copy: Option<String> = db.conn().query_row("SELECT censored_path FROM streams WHERE id=?1", [sid], |r| r.get(0))?;
        if let Some(c) = copy {
            let _ = std::fs::remove_file(c);
            db.conn().execute("UPDATE streams SET censored_path=NULL WHERE id=?1", [sid])?;
        }
    }
    if p.stream_id.is_some() && p.platform == "youtube" {
        db.enqueue("upload_captions", None, &format!("{{\"post\":{id}}}"))?;
    }
    db.log("info", "publish", &format!("Published \"{}\" to {}", p.title, p.platform));
    Ok(format!("Published: {}", res.url))
}

// ---------- scheduled maintenance ----------

/// Every 10 minutes: stats every 6 h, trend report and watchlist once a day.
pub async fn scheduler_loop(db: Arc<Db>) {
    loop {
        let due = |kind: &str, hours: i64| -> bool {
            let last: Option<String> = db.conn().query_row("SELECT MAX(created_at) FROM jobs WHERE kind=?1", [kind], |r| r.get(0)).ok().flatten();
            last.and_then(|l| chrono::DateTime::parse_from_rfc3339(&l).ok()).is_none_or(|t| chrono::Utc::now().signed_duration_since(t).num_hours() >= hours)
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
    use super::*;

    #[test]
    fn thumbnails_avoid_nudity_or_blur() {
        let r = |a: f64, b: f64| nudity::Region { start: a, end: b, bbox: [0.0; 4] };
        // Clean moment: the usual times.
        assert_eq!(thumb_times(100.0, 160.0, &[]), (vec![135.0, 150.0, 159.0, 102.0], false));
        // Nudity around 35 s in: those times are skipped, and none read within 2 s of it.
        let (t, blur) = thumb_times(100.0, 160.0, &[r(30.0, 40.0)]);
        assert!(!blur && !t.is_empty());
        assert!(t.iter().all(|x| x - 100.0 + 2.0 < 30.0 || x - 100.0 - 2.0 > 40.0), "{t:?}");
        // Nudity all the way through: pixelate.
        let (t, blur) = thumb_times(100.0, 160.0, &[r(0.0, 60.0)]);
        assert!(blur && t.len() == 4);
    }

    #[test]
    fn censored_key_changes_with_its_inputs() {
        let s = Settings::default();
        let k = censored_key("x.mp4", &[(1.0, 2.0)], &[], &s);
        assert_eq!(k.len(), 16);
        assert_eq!(k, censored_key("x.mp4", &[(1.0, 2.0)], &[], &s));
        assert_ne!(k, censored_key("x.mp4", &[(1.0, 2.5)], &[], &s));
        assert_ne!(k, censored_key("x.mp4", &[(1.0, 2.0)], &[(5.0, 9.0)], &s));
        assert_ne!(k, censored_key("y.mp4", &[(1.0, 2.0)], &[], &s));
        let other = Settings { censor_style: format!("{}x", s.censor_style), ..Settings::default() };
        assert_ne!(k, censored_key("x.mp4", &[(1.0, 2.0)], &[], &other));
    }

    #[test]
    fn hms_formats() {
        assert_eq!(super::hms(12.02), "0:12");
        assert_eq!(super::hms(7265.0), "2:01:05");
    }
}
