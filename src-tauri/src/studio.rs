//! Tauri commands for Clip Studio, Thumbnail Studio, publishing, analytics and research.

use crate::{analysis, kick, pipeline, posts, settings::Settings, tools, AppState};
use serde::Serialize;
use serde_json::Value;
use tauri::State;

type Res<T> = Result<T, String>;

fn e(err: anyhow::Error) -> String {
    format!("{err:#}")
}

fn q(err: rusqlite::Error) -> String {
    err.to_string()
}

// ---------- engine ----------

#[derive(Serialize)]
pub struct Engine {
    status: tools::EngineStatus,
    claude: bool,
    installing: bool,
    installing_speakers: bool,
}

#[tauri::command]
pub fn engine_status(st: State<AppState>) -> Res<Engine> {
    let model = Settings::load(&st.db).whisper_model;
    let installing: i64 = st.db.conn().query_row("SELECT COUNT(*) FROM jobs WHERE kind='install_whisper' AND status IN ('queued','running')", [], |r| r.get(0)).map_err(q)?;
    let speakers: i64 = st.db.conn().query_row("SELECT COUNT(*) FROM jobs WHERE kind='install_speakers' AND status IN ('queued','running')", [], |r| r.get(0)).map_err(q)?;
    Ok(Engine { status: tools::status(&st.data_dir, &model), claude: crate::llm::available(), installing: installing > 0, installing_speakers: speakers > 0 })
}

#[tauri::command]
pub fn install_speakers(st: State<AppState>) -> Res<i64> {
    st.db.enqueue("install_speakers", None, "{}").map_err(e)
}

#[tauri::command]
pub fn install_engine(st: State<AppState>) -> Res<i64> {
    st.db.enqueue("install_whisper", None, "{}").map_err(e)
}

/// Re-runs transcription (and everything after it) for a recording.
#[tauri::command]
pub fn process_stream(st: State<AppState>, id: i64) -> Res<i64> {
    let has_transcript: i64 = st.db.conn().query_row("SELECT COUNT(*) FROM transcripts WHERE stream_id=?1 AND engine>=2", [id], |r| r.get(0)).map_err(q)?;
    st.db.enqueue(if has_transcript > 0 { "analyze" } else { "transcribe" }, Some(id), "{}").map_err(e)
}

// ---------- clips ----------

#[derive(Serialize)]
pub struct Clip {
    id: i64,
    stream_id: i64,
    stream_title: String,
    start_s: f64,
    end_s: f64,
    score: f64,
    scores: Value,
    category: Option<String>,
    title: Option<String>,
    hook: Option<String>,
    reason: Option<String>,
    transcript: Option<String>,
    scored_by: String,
    status: String,
    render_status: String,
    render_path: Option<String>,
    cover_path: Option<String>,
    error: Option<String>,
    posts: Vec<(String, String)>, // (platform, status)
    words: Vec<analysis::Word>,
}

#[tauri::command]
pub fn list_clips(st: State<AppState>) -> Res<Vec<Clip>> {
    let conn = st.db.conn();
    let mut stmt = conn
        .prepare(
            "SELECT c.id,c.stream_id,s.title,c.start_s,c.end_s,c.score,c.scores,c.category,c.title,c.hook,c.reason,c.transcript,c.scored_by,
             c.status,c.render_status,c.render_path,c.cover_path,c.error,
             (SELECT GROUP_CONCAT(platform || ':' || status) FROM posts WHERE clip_id=c.id), c.words
             FROM clips c JOIN streams s ON s.id=c.stream_id ORDER BY c.stream_id DESC, c.score DESC",
        )
        .map_err(q)?;
    let rows = stmt
        .query_map([], |r| {
            let posts: Option<String> = r.get(18)?;
            Ok(Clip {
                id: r.get(0)?, stream_id: r.get(1)?, stream_title: r.get(2)?, start_s: r.get(3)?, end_s: r.get(4)?, score: r.get(5)?,
                scores: serde_json::from_str(&r.get::<_, String>(6)?).unwrap_or(Value::Null), category: r.get(7)?, title: r.get(8)?,
                hook: r.get(9)?, reason: r.get(10)?, transcript: r.get(11)?, scored_by: r.get(12)?, status: r.get(13)?,
                render_status: r.get(14)?, render_path: r.get(15)?, cover_path: r.get(16)?, error: r.get(17)?,
                posts: posts.unwrap_or_default().split(',').filter(|x| !x.is_empty()).filter_map(|x| x.split_once(':')).map(|(a, b)| (a.into(), b.into())).collect(),
                words: r.get::<_, Option<String>>(19)?.and_then(|w| serde_json::from_str(&w).ok()).unwrap_or_default(),
            })
        })
        .map_err(q)?;
    rows.collect::<Result<_, _>>().map_err(q)
}

#[tauri::command]
pub async fn approve_clip(st: State<'_, AppState>, id: i64) -> Res<Vec<String>> {
    posts::approve_clip(&st.db, id).await.map_err(e)
}

#[tauri::command]
pub fn reject_clip(st: State<AppState>, id: i64) -> Res<()> {
    st.db.conn().execute("UPDATE clips SET status='rejected' WHERE id=?1", [id]).map_err(q)?;
    st.db.conn().execute("DELETE FROM posts WHERE clip_id=?1 AND status IN ('draft','failed')", [id]).map_err(q)?;
    Ok(())
}

#[tauri::command]
pub fn restore_clip(st: State<AppState>, id: i64) -> Res<()> {
    st.db.conn().execute("UPDATE clips SET status='suggested' WHERE id=?1", [id]).map_err(q)?;
    Ok(())
}

/// Clips you edit by hand can be shorter or longer than the ones found automatically.
const EDIT_MIN: f64 = 5.0;
const EDIT_MAX: f64 = 180.0;

/// Edits trims/title/hook and re-renders.
#[tauri::command]
pub fn edit_clip(st: State<AppState>, id: i64, start: f64, end: f64, title: String, hook: String) -> Res<()> {
    let len = end - start;
    if !(EDIT_MIN - 0.01..=EDIT_MAX + 0.01).contains(&len) || start < 0.0 {
        return Err(format!("clips can be {EDIT_MIN:.0}–{EDIT_MAX:.0} s long (this would be {len:.0} s)"));
    }
    let (sid, old_s, old_e, dur): (i64, f64, f64, Option<f64>) = st.db.conn().query_row(
        "SELECT c.stream_id, c.start_s, c.end_s, s.duration_s FROM clips c JOIN streams s ON s.id=c.stream_id WHERE c.id=?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    ).map_err(q)?;
    if dur.is_some_and(|d| end > d + 0.5) {
        return Err("the clip can't end after the recording does".into());
    }
    let own = pipeline::clip_words(&st.db, id, sid, old_s, old_e);
    let transcript = pipeline::load_words(&st.db, sid).unwrap_or_default();
    pipeline::save_clip_words(&st.db, id, &analysis::retrim(&own, (old_s, old_e), &transcript, start, end)).map_err(e)?;
    st.db.conn().execute(
        "UPDATE clips SET start_s=?2, end_s=?3, title=?4, hook=?5 WHERE id=?1",
        rusqlite::params![id, start, end, title.trim(), hook.trim()],
    ).map_err(q)?;
    posts::queue_render(&st.db, id).map_err(e)
}

fn own_words(st: &AppState, id: i64) -> Res<Vec<analysis::Word>> {
    let (sid, start, end): (i64, f64, f64) = st.db.conn().query_row("SELECT stream_id,start_s,end_s FROM clips WHERE id=?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map_err(q)?;
    Ok(pipeline::clip_words(&st.db, id, sid, start, end))
}

/// Replaces the captions, one text per speaker turn, and re-renders.
#[tauri::command]
pub fn edit_clip_text(st: State<AppState>, id: i64, turns: Vec<String>) -> Res<()> {
    let words = analysis::retext(&own_words(&st, id)?, &turns).map_err(e)?;
    pipeline::save_clip_words(&st.db, id, &words).map_err(e)?;
    posts::queue_render(&st.db, id).map_err(e)
}

/// Bleeps a word the censor missed, or un-bleeps one it got wrong. Applies on the next render.
#[tauri::command]
pub fn toggle_bleep(st: State<AppState>, id: i64, index: usize) -> Res<()> {
    let mut words = own_words(&st, id)?;
    let w = words.get_mut(index).ok_or("that word no longer exists")?;
    w.b = !w.b;
    pipeline::save_clip_words(&st.db, id, &words).map_err(e)
}

/// The recording's transcript as readable lines, for the Clip Picker.
#[tauri::command]
pub fn stream_transcript(st: State<AppState>, id: i64) -> Res<Vec<analysis::Line>> {
    Ok(analysis::lines(&pipeline::load_words(&st.db, id).map_err(e)?))
}

/// A clip you picked yourself: approved straight away and rendered.
#[tauri::command]
pub fn create_clip(st: State<AppState>, stream_id: i64, start: f64, end: f64, title: String) -> Res<i64> {
    create_manual_clip(&st.db, stream_id, start, end, &title).map_err(e)
}

pub fn create_manual_clip(db: &crate::db::Db, stream_id: i64, start: f64, end: f64, title: &str) -> anyhow::Result<i64> {
    let len = end - start;
    if !(EDIT_MIN - 0.01..=EDIT_MAX + 0.01).contains(&len) || start < 0.0 {
        anyhow::bail!("clips can be {EDIT_MIN:.0}–{EDIT_MAX:.0} s long (this would be {len:.0} s)");
    }
    let dur: Option<f64> = db.conn().query_row("SELECT duration_s FROM streams WHERE id=?1", [stream_id], |r| r.get(0))?;
    if dur.is_some_and(|d| end > d + 0.5) {
        anyhow::bail!("the clip can't end after the recording does");
    }
    let words = analysis::slice_words(&pipeline::load_words(db, stream_id).unwrap_or_default(), start, end);
    let title = if title.trim().is_empty() { analysis::auto_title(&words, start + len / 2.0) } else { title.trim().to_string() };
    let id = {
        let conn = db.conn();
        conn.execute(
            "INSERT INTO clips(stream_id,start_s,end_s,score,category,title,transcript,scored_by,status,words)
             VALUES(?1,?2,?3,100,'moment',?4,?5,'manual','approved',?6)",
            rusqlite::params![stream_id, start, end, title, analysis::words_text(&words), serde_json::to_string(&words)?],
        )?;
        conn.last_insert_rowid()
    };
    posts::queue_render(db, id)?;
    Ok(id)
}

#[derive(Serialize)]
pub struct Cuts {
    /// Found automatically: stretches with no talking and little going on.
    auto: Vec<(f64, f64)>,
    /// Marked by you in the Clip Picker.
    manual: Vec<(f64, f64)>,
}

#[tauri::command]
pub fn stream_cuts(st: State<AppState>, id: i64) -> Res<Cuts> {
    let auto: Option<String> = st.db.conn().query_row("SELECT cuts FROM streams WHERE id=?1", [id], |r| r.get(0)).map_err(q)?;
    Ok(Cuts { auto: auto.and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default(), manual: pipeline::manual_cuts(&st.db, id) })
}

/// Adds or removes a stretch you want cut from the full upload.
#[tauri::command]
pub fn set_manual_cuts(st: State<AppState>, id: i64, cuts: Vec<(f64, f64)>) -> Res<()> {
    if cuts.iter().any(|(a, b)| b <= a || *a < 0.0) {
        return Err("each cut needs an end after its start".into());
    }
    let merged = crate::censor::merge(cuts);
    st.db.conn().execute("UPDATE streams SET manual_cuts=?2, censored_path=NULL WHERE id=?1", rusqlite::params![id, serde_json::to_string(&merged).map_err(|err| err.to_string())?]).map_err(q)?;
    Ok(())
}

/// Copies a finished clip to where you chose.
#[tauri::command]
pub fn download_clip(st: State<AppState>, id: i64, dest: String) -> Res<()> {
    let path: Option<String> = st.db.conn().query_row("SELECT render_path FROM clips WHERE id=?1", [id], |r| r.get(0)).map_err(q)?;
    let path = path.ok_or("render the clip first")?;
    std::fs::copy(&path, &dest).map_err(|err| format!("couldn't save the clip: {err}"))?;
    Ok(())
}

#[tauri::command]
pub fn render_clip(st: State<AppState>, id: i64) -> Res<()> {
    posts::queue_render(&st.db, id).map_err(e)
}

// ---------- Kick VODs ----------

#[derive(Serialize)]
pub struct KickVod {
    #[serde(flatten)]
    vod: kick::Vod,
    /// The library entry for this VOD, if it's already there.
    stream_id: Option<i64>,
}

/// Past broadcasts on your Kick channel, newest first.
#[tauri::command]
pub async fn list_kick_vods(st: State<'_, AppState>) -> Res<Vec<KickVod>> {
    let slug = Settings::load(&st.db).kick_channel_slug.ok_or("Set your Kick channel name first")?;
    let vods = kick::list_vods(&slug).await.map_err(e)?;
    let conn = st.db.conn();
    Ok(vods
        .into_iter()
        .map(|vod| {
            let stream_id = conn.query_row("SELECT id FROM streams WHERE kick_vod_id=?1", [&vod.id], |r| r.get(0)).ok();
            KickVod { vod, stream_id }
        })
        .collect())
}

#[tauri::command]
pub fn import_kick_vod(st: State<AppState>, vod: kick::Vod) -> Res<i64> {
    kick::add_vod(&st.db, &vod).map_err(e)
}

/// Imports a VOD from a pasted kick.com link.
#[tauri::command]
pub async fn import_kick_link(st: State<'_, AppState>, link: String) -> Res<i64> {
    let vod = kick::vod_from_link(&link).await.map_err(e)?;
    kick::add_vod(&st.db, &vod).map_err(e)
}

// ---------- thumbnails ----------

#[derive(Serialize)]
pub struct Thumb {
    id: i64,
    stream_id: i64,
    stream_title: String,
    path: String,
    rank: i64,
    concept: String,
    selected: bool,
}

#[tauri::command]
pub fn list_thumbnails(st: State<AppState>) -> Res<Vec<Thumb>> {
    let conn = st.db.conn();
    let mut stmt = conn
        .prepare("SELECT t.id,t.stream_id,s.title,t.path,t.rank,t.concept,t.selected FROM thumbnails t JOIN streams s ON s.id=t.stream_id ORDER BY t.stream_id DESC, t.rank")
        .map_err(q)?;
    let rows = stmt
        .query_map([], |r| Ok(Thumb { id: r.get(0)?, stream_id: r.get(1)?, stream_title: r.get(2)?, path: r.get(3)?, rank: r.get(4)?, concept: r.get(5)?, selected: r.get::<_, i64>(6)? == 1 }))
        .map_err(q)?;
    rows.collect::<Result<_, _>>().map_err(q)
}

#[tauri::command]
pub fn select_thumbnail(st: State<AppState>, id: i64) -> Res<()> {
    let conn = st.db.conn();
    conn.execute("UPDATE thumbnails SET selected=0 WHERE stream_id=(SELECT stream_id FROM thumbnails WHERE id=?1)", [id]).map_err(q)?;
    conn.execute("UPDATE thumbnails SET selected=1 WHERE id=?1", [id]).map_err(q)?;
    Ok(())
}

#[tauri::command]
pub fn regenerate_thumbnails(st: State<AppState>, stream_id: i64) -> Res<i64> {
    st.db.enqueue("thumbnails", Some(stream_id), "{}").map_err(e)
}

// ---------- posts ----------

#[tauri::command]
pub fn list_posts(st: State<AppState>) -> Res<Vec<posts::Post>> {
    posts::list_posts(&st.db).map_err(e)
}

#[tauri::command]
pub fn update_post(st: State<AppState>, id: i64, title: String, description: String, tags: Vec<String>, privacy: String) -> Res<()> {
    posts::update_post(&st.db, id, &title, &description, &tags, &privacy).map_err(e)
}

#[tauri::command]
pub fn schedule_post(st: State<AppState>, id: i64, at: Option<String>) -> Res<()> {
    posts::schedule(&st.db, id, at.as_deref()).map_err(e)
}

#[tauri::command]
pub fn unschedule_post(st: State<AppState>, id: i64) -> Res<()> {
    posts::unschedule(&st.db, id).map_err(e)
}

#[tauri::command]
pub fn delete_failed_posts(st: State<AppState>) -> Res<usize> {
    posts::delete_failed(&st.db).map_err(e)
}

#[tauri::command]
pub fn delete_post(st: State<AppState>, id: i64) -> Res<()> {
    posts::delete_post(&st.db, id).map_err(e)
}

#[tauri::command]
pub async fn draft_full_video(st: State<'_, AppState>, stream_id: i64) -> Res<i64> {
    posts::draft_full_video(&st.db, stream_id).await.map_err(e)
}

// ---------- analytics + research ----------

#[tauri::command]
pub fn analytics(st: State<AppState>) -> Res<posts::Analytics> {
    posts::analytics(&st.db).map_err(e)
}

#[tauri::command]
pub fn refresh_stats(st: State<AppState>) -> Res<i64> {
    st.db.enqueue("refresh_stats", None, "{}").map_err(e)
}

#[derive(Serialize)]
pub struct TrendReport {
    keywords: String,
    created_at: String,
    report: Value,
}

#[tauri::command]
pub fn latest_trend_report(st: State<AppState>) -> Res<Option<TrendReport>> {
    let r = st.db.conn().query_row("SELECT keywords, created_at, report FROM trend_reports ORDER BY id DESC LIMIT 1", [], |r| {
        Ok(TrendReport { keywords: r.get(0)?, created_at: r.get(1)?, report: serde_json::from_str(&r.get::<_, String>(2)?).unwrap_or(Value::Null) })
    });
    match r {
        Ok(x) => Ok(Some(x)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(err) => Err(q(err)),
    }
}

#[tauri::command]
pub fn run_trend_report(st: State<AppState>) -> Res<i64> {
    if Settings::load(&st.db).trend_keywords.is_empty() {
        return Err("Add at least one keyword first".into());
    }
    st.db.enqueue("trend_report", None, "{}").map_err(e)
}

#[derive(Serialize)]
pub struct Watched {
    id: i64,
    handle: String,
    channel_id: Option<String>,
    title: Option<String>,
    subscribers: Option<i64>,
    summary: Value,
    checked_at: Option<String>,
    error: Option<String>,
}

#[tauri::command]
pub fn list_watchlist(st: State<AppState>) -> Res<Vec<Watched>> {
    let conn = st.db.conn();
    let mut stmt = conn.prepare("SELECT id,handle,channel_id,title,subscribers,summary,checked_at,error FROM watchlist ORDER BY id").map_err(q)?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Watched {
                id: r.get(0)?, handle: r.get(1)?, channel_id: r.get(2)?, title: r.get(3)?, subscribers: r.get(4)?,
                summary: serde_json::from_str(&r.get::<_, String>(5)?).unwrap_or(Value::Null), checked_at: r.get(6)?, error: r.get(7)?,
            })
        })
        .map_err(q)?;
    rows.collect::<Result<_, _>>().map_err(q)
}

#[tauri::command]
pub fn add_watch(st: State<AppState>, handle: String) -> Res<()> {
    let h = handle.trim().trim_start_matches("https://www.youtube.com/").trim_start_matches("https://youtube.com/").trim_matches('/').to_string();
    if h.is_empty() || h.len() > 100 {
        return Err("Enter a YouTube @handle or channel id".into());
    }
    st.db.conn().execute("INSERT OR IGNORE INTO watchlist(handle) VALUES(?1)", [&h]).map_err(q)?;
    st.db.enqueue("refresh_watchlist", None, "{}").map_err(e)?;
    Ok(())
}

#[tauri::command]
pub fn remove_watch(st: State<AppState>, id: i64) -> Res<()> {
    st.db.conn().execute("DELETE FROM watchlist WHERE id=?1", [id]).map_err(q)?;
    Ok(())
}

#[tauri::command]
pub fn refresh_watchlist(st: State<AppState>) -> Res<i64> {
    st.db.enqueue("refresh_watchlist", None, "{}").map_err(e)
}

/// Removes intermediate work files (audio, partial renders). Rendered clips are kept.
#[tauri::command]
pub fn clean_temp(st: State<AppState>) -> Res<String> {
    let work = st.data_dir.join("work");
    let size = dir_size(&work);
    if work.exists() {
        std::fs::remove_dir_all(&work).map_err(|err| err.to_string())?;
    }
    Ok(format!("Freed {:.1} MB", size as f64 / 1_048_576.0))
}

fn dir_size(p: &std::path::Path) -> u64 {
    std::fs::read_dir(p)
        .map(|rd| rd.flatten().map(|e| if e.path().is_dir() { dir_size(&e.path()) } else { e.metadata().map(|m| m.len()).unwrap_or(0) }).sum())
        .unwrap_or(0)
}

#[derive(Serialize)]
pub struct Disk {
    path: String,
    free_gb: f64,
    total_gb: f64,
}

/// Free space where recordings are processed and clips are written.
#[tauri::command]
pub fn disk_status(st: State<AppState>) -> Vec<Disk> {
    let s = Settings::load(&st.db);
    let mut paths = vec![st.data_dir.to_string_lossy().to_string()];
    paths.extend(s.output_dir.clone());
    paths.extend(s.recording_dir.clone());
    let mut seen = Vec::new();
    paths
        .into_iter()
        .filter_map(|p| {
            let (free, total) = crate::media::disk_space(&p)?;
            let root: String = p.chars().take(3).collect();
            (!seen.contains(&root)).then(|| { seen.push(root.clone()); Disk { path: root, free_gb: free as f64 / 1e9, total_gb: total as f64 / 1e9 } })
        })
        .collect()
}
