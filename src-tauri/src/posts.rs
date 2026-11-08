//! Clip approval, draft posts, scheduling and the publishing queue.
//! Review-first: approving a clip creates *draft* posts; nothing uploads until a post is
//! scheduled or published by you, unless full automation is switched on in Settings.

use crate::{db::Db, llm, research, settings::{Settings, Socials}};
use anyhow::{anyhow, bail, Result};
use chrono::{DateTime, Duration, Timelike, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Serialize, Clone)]
pub struct Post {
    pub id: i64,
    pub clip_id: Option<i64>,
    pub stream_id: Option<i64>,
    pub platform: String,
    pub title: String,
    pub description: String,
    pub tags: String,
    pub privacy: String,
    pub scheduled_at: Option<String>,
    pub status: String,
    pub progress: f64,
    pub remote_id: Option<String>,
    pub url: Option<String>,
    pub error: Option<String>,
    pub published_at: Option<String>,
    pub created_at: String,
    pub media_title: Option<String>,
    pub cover: Option<String>,
    pub views: Option<i64>,
    pub likes: Option<i64>,
    pub comments: Option<i64>,
}

const POST_SELECT: &str = "SELECT p.id,p.clip_id,p.stream_id,p.platform,p.title,p.description,p.tags,p.privacy,p.scheduled_at,p.status,p.progress,
    p.remote_id,p.url,p.error,p.published_at,p.created_at,
    COALESCE(c.title, s.title), COALESCE(c.cover_path, (SELECT path FROM thumbnails t WHERE t.stream_id=p.stream_id AND t.selected=1)),
    st.views, st.likes, st.comments
    FROM posts p LEFT JOIN clips c ON c.id=p.clip_id LEFT JOIN streams s ON s.id=p.stream_id
    LEFT JOIN post_stats st ON st.post_id=p.id AND st.ts=(SELECT MAX(ts) FROM post_stats WHERE post_id=p.id)";

fn post_row(r: &rusqlite::Row) -> rusqlite::Result<Post> {
    Ok(Post {
        id: r.get(0)?, clip_id: r.get(1)?, stream_id: r.get(2)?, platform: r.get(3)?, title: r.get(4)?, description: r.get(5)?,
        tags: r.get(6)?, privacy: r.get(7)?, scheduled_at: r.get(8)?, status: r.get(9)?, progress: r.get(10)?, remote_id: r.get(11)?,
        url: r.get(12)?, error: r.get(13)?, published_at: r.get(14)?, created_at: r.get(15)?, media_title: r.get(16)?, cover: r.get(17)?,
        views: r.get(18)?, likes: r.get(19)?, comments: r.get(20)?,
    })
}

pub fn get_post(db: &Db, id: i64) -> Result<Post> {
    Ok(db.conn().query_row(&format!("{POST_SELECT} WHERE p.id=?1"), [id], post_row)?)
}

pub fn list_posts(db: &Db) -> Result<Vec<Post>> {
    let conn = db.conn();
    let mut st = conn.prepare(&format!("{POST_SELECT} ORDER BY COALESCE(p.scheduled_at, p.published_at, p.created_at) DESC"))?;
    let rows = st.query_map([], post_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// File, duration and thumbnail for a post.
pub fn media_for(db: &Db, p: &Post) -> Result<(PathBuf, f64, Option<PathBuf>)> {
    let conn = db.conn();
    if let Some(cid) = p.clip_id {
        let (path, status, start, end): (Option<String>, String, f64, f64) =
            conn.query_row("SELECT render_path, render_status, start_s, end_s FROM clips WHERE id=?1", [cid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
        let path = path.filter(|_| status == "done").ok_or_else(|| anyhow!("the clip hasn't been rendered yet"))?;
        return Ok((PathBuf::from(path), end - start, None));
    }
    let sid = p.stream_id.ok_or_else(|| anyhow!("post has no media"))?;
    // The bleeped copy when there is one.
    let (path, dur): (Option<String>, Option<f64>) = conn.query_row("SELECT COALESCE(censored_path, file_path), duration_s FROM streams WHERE id=?1", [sid], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let thumb: Option<String> = conn.query_row("SELECT path FROM thumbnails WHERE stream_id=?1 AND selected=1", [sid], |r| r.get(0)).ok();
    Ok((PathBuf::from(path.ok_or_else(|| anyhow!("recording file missing"))?), dur.unwrap_or(0.0), thumb.map(PathBuf::from)))
}

pub fn queue_render(db: &Db, clip_id: i64) -> Result<()> {
    db.conn().execute("UPDATE clips SET render_status='queued', error=NULL WHERE id=?1", [clip_id])?;
    db.enqueue("render", None, &format!("{{\"clip\":{clip_id}}}"))?;
    Ok(())
}

/// Platform limits: returns why a clip of `secs` can't go to `platform`, if it can't.
pub fn duration_problem(platform: &str, secs: f64) -> Option<String> {
    match platform {
        "youtube" if secs > 180.0 => Some(format!("YouTube Shorts allow up to 3 minutes; this clip is {secs:.0} s")),
        "facebook" if secs > 90.0 => Some(format!("Facebook Reels via API allow 3–90 s; this clip is {secs:.0} s, so it isn't sent there")),
        _ => None,
    }
}

fn link(base: &str, value: &str) -> String {
    let v = value.trim();
    if v.starts_with("http") { v.to_string() } else { format!("{base}{}", v.trim_start_matches('@')) }
}

/// The links section of a description. A handle or a full URL both work.
pub fn links_text(s: &Socials) -> String {
    let rows = [
        ("Kick", "https://kick.com/", &s.kick),
        ("TikTok", "https://www.tiktok.com/@", &s.tiktok),
        ("Instagram", "https://www.instagram.com/", &s.instagram),
        ("X", "https://x.com/", &s.x),
        ("Discord", "https://discord.gg/", &s.discord),
    ];
    let lines: Vec<String> = rows.iter().filter(|(_, _, v)| !v.trim().is_empty()).map(|(n, b, v)| format!("{n}: {}", link(b, v))).collect();
    if lines.is_empty() { String::new() } else { format!("Follow me:\n{}", lines.join("\n")) }
}

/// Socials from Settings; Kick and TikTok fall back to the connected accounts.
fn socials(db: &Db) -> Socials {
    let s = Settings::load(db);
    let mut out = s.socials;
    if out.kick.trim().is_empty() {
        out.kick = s.kick_channel_slug.unwrap_or_default();
    }
    if out.tiktok.trim().is_empty() {
        out.tiktok = db.conn().query_row("SELECT display_name FROM accounts WHERE platform='tiktok'", [], |r| r.get(0)).unwrap_or_default();
    }
    out
}

/// A YouTube description with the links section on the end.
fn with_links(db: &Db, description: &str) -> String {
    let links = links_text(&socials(db));
    [description.trim(), links.as_str()].iter().filter(|p| !p.is_empty()).copied().collect::<Vec<_>>().join("\n\n")
}

fn default_tags(db: &Db, category: &str) -> Vec<String> {
    let mut tags = vec![category.to_string()];
    if let Ok(Some(r)) = db.conn().query_row("SELECT report FROM trend_reports ORDER BY id DESC LIMIT 1", [], |r| r.get::<_, String>(0)).map(Some) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&r) {
            tags.extend(v["top_hashtags"].as_array().into_iter().flatten().filter_map(|p| p[0].as_str().map(String::from)).take(4));
        }
    }
    tags.push("kick".into());
    tags.dedup();
    tags
}

/// Marks a clip approved and creates draft posts for the enabled platforms.
pub async fn approve_clip(db: &Db, clip_id: i64) -> Result<Vec<String>> {
    let (title, transcript, category, start, end, render_status): (Option<String>, Option<String>, Option<String>, f64, f64, String) = db.conn().query_row(
        "SELECT title, transcript, category, start_s, end_s, render_status FROM clips WHERE id=?1",
        [clip_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
    )?;
    db.conn().execute("UPDATE clips SET status='approved' WHERE id=?1", [clip_id])?;
    if render_status == "none" || render_status == "error" {
        queue_render(db, clip_id)?;
    }
    let s = Settings::load(db);
    let title = title.unwrap_or_else(|| "Stream highlight".into());
    let category = category.unwrap_or_else(|| "moment".into());
    let mut notes = Vec::new();
    for platform in &s.publish_platforms {
        if !matches!(platform.as_str(), "youtube" | "tiktok") {
            notes.push(format!("{platform}: publishing not available yet"));
            continue;
        }
        if let Some(problem) = duration_problem(platform, end - start) {
            notes.push(problem);
            continue;
        }
        let exists: bool = db.conn().query_row("SELECT COUNT(*) FROM posts WHERE clip_id=?1 AND platform=?2", rusqlite::params![clip_id, platform], |r| r.get::<_, i64>(0))? > 0;
        if exists {
            continue;
        }
        let (t, d, tags) = match (llm::available(), transcript.as_deref()) {
            (true, Some(tx)) => match llm::metadata(platform, tx, &title, false).await {
                Ok(m) => (m.title, m.description, m.tags),
                Err(e) => {
                    notes.push(format!("Claude metadata skipped: {e:#}"));
                    heuristic_meta(db, platform, &title, &category)
                }
            },
            _ => heuristic_meta(db, platform, &title, &category),
        };
        let d = if platform == "youtube" { with_links(db, &d) } else { d };
        db.conn().execute(
            "INSERT INTO posts(clip_id, platform, title, description, tags, privacy) VALUES(?1,?2,?3,?4,?5,?6)",
            rusqlite::params![clip_id, platform, t, d, serde_json::to_string(&tags)?, s.default_privacy],
        )?;
    }
    Ok(notes)
}

fn heuristic_meta(db: &Db, platform: &str, title: &str, category: &str) -> (String, String, Vec<String>) {
    let tags = default_tags(db, category);
    let clean = title.trim_matches('"').to_string();
    match platform {
        "youtube" => (format!("{clean} #Shorts"), "From my Kick stream. Full streams and more clips on the channel.".into(), tags),
        _ => (clean, String::new(), tags),
    }
}

/// Title, description, tags and thumbnail wording for a full stream.
#[derive(Serialize, Deserialize, Clone)]
pub struct StreamMeta {
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
    #[serde(default)]
    pub hashtags: Vec<String>,
    pub thumb_text: String,
}

/// `Project Zomboid` → `#ProjectZomboid`.
pub fn hashtag(text: &str) -> String {
    let words: String = text.split_whitespace().map(|w| {
        let w: String = w.chars().filter(|c| c.is_alphanumeric()).collect();
        let mut c = w.chars();
        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
    }).collect();
    format!("#{words}")
}

/// The hashtag line for a full stream: the game, then Claude's, then the ones from Settings,
/// without repeats. YouTube shows the first three above the title.
pub fn hashtag_line(category: Option<&str>, own: &[String], settings: &[String]) -> String {
    let mut seen: Vec<String> = Vec::new();
    for h in category.map(hashtag).into_iter().chain(own.iter().map(|h| hashtag(h))).chain(settings.iter().map(|h| hashtag(h))) {
        if h.len() > 1 && !seen.iter().any(|x| x.eq_ignore_ascii_case(&h)) {
            seen.push(h);
        }
    }
    seen.join(" ")
}

/// Written by Claude once and kept when a key is set, otherwise built from the stream title.
pub async fn stream_meta(db: &Db, stream_id: i64) -> Result<StreamMeta> {
    let (title, started, saved, category): (String, Option<String>, Option<String>, Option<String>) =
        db.conn().query_row("SELECT title, kick_started, meta, category FROM streams WHERE id=?1", [stream_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
    if let Some(m) = saved.and_then(|s| serde_json::from_str(&s).ok()) {
        return Ok(m);
    }
    let highlights: String = db.conn().query_row(
        "SELECT COALESCE(GROUP_CONCAT(transcript, '\n\n'), '') FROM (SELECT transcript FROM clips WHERE stream_id=?1 AND status!='rejected' ORDER BY score DESC LIMIT 5)",
        [stream_id],
        |r| r.get(0),
    )?;
    if llm::available() && !highlights.is_empty() {
        match llm::video_package(&title, category.as_deref(), &highlights).await {
            Ok(p) => {
                let m = StreamMeta { title: p.title, description: p.description, tags: p.tags, hashtags: p.hashtags, thumb_text: p.thumb_text };
                db.conn().execute("UPDATE streams SET meta=?2 WHERE id=?1", rusqlite::params![stream_id, serde_json::to_string(&m)?])?;
                return Ok(m);
            }
            Err(e) => db.log("warn", "metadata", &format!("Claude metadata skipped: {e:#}")),
        }
    }
    let date = started.and_then(|t| DateTime::parse_from_rfc3339(&t).ok()).map(|t| t.format("%B %-d, %Y").to_string());
    let game = category.as_deref().map(|c| format!(" of {c}")).unwrap_or_default();
    let mut tags = default_tags(db, "stream");
    tags.extend(category.clone());
    Ok(StreamMeta {
        description: match date { Some(d) => format!("Full stream{game} from {d}, live on Kick."), None => format!("Full stream{game}, live on Kick.") },
        tags,
        hashtags: Vec::new(),
        thumb_text: crate::render::headline(&title),
        title,
    })
}

/// Creates a draft for uploading the full recording to YouTube.
pub async fn draft_full_video(db: &Db, stream_id: i64) -> Result<i64> {
    let dur: Option<f64> = db.conn().query_row("SELECT duration_s FROM streams WHERE id=?1", [stream_id], |r| r.get(0))?;
    if dur.is_none() {
        bail!("this recording hasn't been read yet");
    }
    let m = stream_meta(db, stream_id).await?;
    let s = Settings::load(db);
    let privacy = s.vod_privacy.clone();
    let category: Option<String> = db.conn().query_row("SELECT category FROM streams WHERE id=?1", [stream_id], |r| r.get(0))?;
    // Worked out first: it needs the database, which `execute` below holds until it returns.
    let tags_line = hashtag_line(category.as_deref(), &m.hashtags, &s.hashtags);
    let description = format!("{}\n\n{tags_line}", with_links(db, &m.description)).trim().to_string();
    db.conn().execute(
        "INSERT INTO posts(stream_id, platform, title, description, tags, privacy) VALUES(?1,'youtube',?2,?3,?4,?5)
         ON CONFLICT DO NOTHING",
        rusqlite::params![stream_id, m.title, description, serde_json::to_string(&m.tags)?, privacy],
    )?;
    Ok(db.conn().query_row("SELECT id FROM posts WHERE stream_id=?1 AND platform='youtube'", [stream_id], |r| r.get(0))?)
}

/// Uploads a finished stream to YouTube if it was marked for that when it went live.
pub async fn auto_upload(db: &Db, stream_id: i64) -> Result<()> {
    let wanted: i64 = db.conn().query_row("SELECT auto_upload FROM streams WHERE id=?1", [stream_id], |r| r.get(0))?;
    if wanted == 0 || !Settings::load(db).auto_upload_vod {
        return Ok(());
    }
    let id = draft_full_video(db, stream_id).await?;
    schedule(db, id, None)?;
    db.conn().execute("UPDATE streams SET auto_upload=0 WHERE id=?1", [stream_id])?;
    Ok(())
}

/// Queues a post for `at` (UTC RFC 3339) or now.
pub fn schedule(db: &Db, post_id: i64, at: Option<&str>) -> Result<()> {
    let p = get_post(db, post_id)?;
    if p.status == "published" || p.status == "uploading" {
        bail!("this post is already {}", p.status);
    }
    let when = match at {
        Some(t) => DateTime::parse_from_rfc3339(t).map_err(|_| anyhow!("invalid time"))?.with_timezone(&Utc),
        None => Utc::now(),
    };
    let ts = when.format("%Y-%m-%dT%H:%M:%SZ").to_string();
    db.conn().execute("UPDATE posts SET status='scheduled', scheduled_at=?2, error=NULL WHERE id=?1", rusqlite::params![post_id, ts])?;
    // Re-scheduling replaces any queued upload for this post.
    db.conn().execute("DELETE FROM jobs WHERE kind='publish' AND payload=?1 AND status='queued'", [format!("{{\"post\":{post_id}}}")])?;
    db.enqueue_at("publish", None, &format!("{{\"post\":{post_id}}}"), &ts)?;
    Ok(())
}

pub fn unschedule(db: &Db, post_id: i64) -> Result<()> {
    db.conn().execute("DELETE FROM jobs WHERE kind='publish' AND payload=?1 AND status='queued'", [format!("{{\"post\":{post_id}}}")])?;
    db.conn().execute("UPDATE posts SET status='draft', scheduled_at=NULL WHERE id=?1 AND status IN ('scheduled','failed')", [post_id])?;
    Ok(())
}

/// Next free slots spaced evenly through the day (e.g. 2/day → every 12 h), on the hour.
pub fn next_slots(taken: &[DateTime<Utc>], per_day: u32, count: usize, now: DateTime<Utc>) -> Vec<DateTime<Utc>> {
    let gap = Duration::hours(24 / per_day.clamp(1, 24) as i64);
    let mut t = (now + Duration::hours(1)).with_minute(0).unwrap().with_second(0).unwrap().with_nanosecond(0).unwrap();
    let mut out = Vec::new();
    while out.len() < count {
        if taken.iter().all(|x| (*x - t).num_minutes().abs() >= gap.num_minutes() / 2) {
            out.push(t);
        }
        t += gap;
    }
    out
}

/// Full-automation path: schedule a clip's draft posts into the next free slots.
pub async fn auto_schedule_clip(db: &Db, clip_id: i64) -> Result<()> {
    let per_day = Settings::load(db).posts_per_day;
    let drafts: Vec<(i64, String)> = {
        let conn = db.conn();
        let mut st = conn.prepare("SELECT id, platform FROM posts WHERE clip_id=?1 AND status='draft'")?;
        let rows = st.query_map([clip_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<Result<_, _>>()?
    };
    for (id, platform) in drafts {
        let taken: Vec<DateTime<Utc>> = {
            let conn = db.conn();
            let mut st = conn.prepare("SELECT scheduled_at FROM posts WHERE platform=?1 AND status='scheduled' AND scheduled_at IS NOT NULL")?;
            let rows = st.query_map([&platform], |r| r.get::<_, String>(0))?;
            rows.filter_map(|r| r.ok()).filter_map(|s| DateTime::parse_from_rfc3339(&s).ok().map(|d| d.with_timezone(&Utc))).collect()
        };
        let slot = next_slots(&taken, per_day, 1, Utc::now())[0];
        schedule(db, id, Some(&slot.to_rfc3339()))?;
    }
    Ok(())
}

pub fn update_post(db: &Db, id: i64, title: &str, description: &str, tags: &[String], privacy: &str) -> Result<()> {
    if !["private", "unlisted", "public"].contains(&privacy) {
        bail!("invalid privacy");
    }
    let n = db.conn().execute(
        "UPDATE posts SET title=?2, description=?3, tags=?4, privacy=?5 WHERE id=?1 AND status IN ('draft','scheduled','failed','published')",
        rusqlite::params![id, title.trim(), description, serde_json::to_string(tags)?, privacy],
    )?;
    if n == 0 {
        bail!("posts can't be edited while they upload");
    }
    // Already on YouTube: send the change there too.
    let (status, platform): (String, String) = db.conn().query_row("SELECT status, platform FROM posts WHERE id=?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    if status == "published" {
        if platform != "youtube" {
            bail!("{platform} posts can't be changed after publishing");
        }
        db.enqueue("update_video", None, &format!("{{\"post\":{id}}}"))?;
    }
    Ok(())
}

/// Deletes every failed post (they never reached the platform). Returns how many.
pub fn delete_failed(db: &Db) -> Result<usize> {
    let ids: Vec<i64> = {
        let conn = db.conn();
        let mut st = conn.prepare("SELECT id FROM posts WHERE status='failed'")?;
        let rows = st.query_map([], |r| r.get(0))?;
        rows.collect::<Result<_, _>>()?
    };
    for id in &ids {
        delete_post(db, *id)?;
    }
    Ok(ids.len())
}

pub fn delete_post(db: &Db, id: i64) -> Result<()> {
    unschedule(db, id)?;
    db.conn().execute("DELETE FROM posts WHERE id=?1 AND status!='published' AND status!='uploading'", [id])?;
    Ok(())
}

/// Totals for the analytics page, all from stored platform numbers.
#[derive(Serialize)]
pub struct Analytics {
    pub published: i64,
    pub total_views: i64,
    pub total_likes: i64,
    pub total_comments: i64,
    pub by_platform: Vec<(String, i64, i64)>,  // platform, posts, views
    pub by_category: Vec<(String, f64, usize)>, // category, avg views, posts
    pub by_hour: Vec<(u32, f64, i64)>,          // UTC hour published, avg views, posts
    pub series: Vec<(String, i64)>,             // day, total views that day (latest snapshot per post)
    pub last_refresh: Option<String>,
}

pub fn analytics(db: &Db) -> Result<Analytics> {
    let latest = "SELECT p.id, p.platform, p.published_at, s.views, s.likes, s.comments FROM posts p
        LEFT JOIN post_stats s ON s.post_id=p.id AND s.ts=(SELECT MAX(ts) FROM post_stats WHERE post_id=p.id) WHERE p.status='published'";
    let rows: Vec<(i64, String, Option<String>, Option<i64>, Option<i64>, Option<i64>)> = {
        let conn = db.conn();
        let mut st = conn.prepare(latest)?;
        let r = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?.collect::<Result<_, _>>()?;
        r
    };
    let mut by_platform: std::collections::BTreeMap<String, (i64, i64)> = Default::default();
    let mut hours: std::collections::BTreeMap<u32, (f64, i64)> = Default::default();
    for (_, plat, published, views, ..) in &rows {
        let e = by_platform.entry(plat.clone()).or_default();
        e.0 += 1;
        e.1 += views.unwrap_or(0);
        if let (Some(p), Some(v)) = (published, views) {
            if let Ok(t) = DateTime::parse_from_rfc3339(p) {
                let h = hours.entry(t.with_timezone(&Utc).hour()).or_default();
                h.0 += *v as f64;
                h.1 += 1;
            }
        }
    }
    let (series, last_refresh): (Vec<(String, i64)>, Option<String>) = {
        let conn = db.conn();
        let mut st = conn.prepare("SELECT substr(ts,1,10) d, SUM(views) FROM post_stats s WHERE ts=(SELECT MAX(ts) FROM post_stats x WHERE x.post_id=s.post_id AND substr(x.ts,1,10)=substr(s.ts,1,10)) GROUP BY d ORDER BY d")?;
        let series = st.query_map([], |r| Ok((r.get(0)?, r.get::<_, Option<i64>>(1)?.unwrap_or(0))))?.collect::<Result<_, _>>()?;
        let last = conn.query_row("SELECT MAX(ts) FROM post_stats", [], |r| r.get(0))?;
        (series, last)
    };
    let mut by_category: Vec<(String, f64, usize)> = research::category_performance(db)?.into_iter().map(|(k, (a, c))| (k, a.round(), c)).collect();
    by_category.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    Ok(Analytics {
        published: rows.len() as i64,
        total_views: rows.iter().map(|r| r.3.unwrap_or(0)).sum(),
        total_likes: rows.iter().map(|r| r.4.unwrap_or(0)).sum(),
        total_comments: rows.iter().map(|r| r.5.unwrap_or(0)).sum(),
        by_platform: by_platform.into_iter().map(|(k, (n, v))| (k, n, v)).collect(),
        by_category,
        by_hour: hours.into_iter().map(|(h, (sum, n))| (h, (sum / n as f64).round(), n)).collect(),
        series,
        last_refresh,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_are_spaced_and_skip_taken_times() {
        let now = DateTime::parse_from_rfc3339("2026-10-30T10:20:00Z").unwrap().with_timezone(&Utc);
        let s = next_slots(&[], 2, 2, now);
        assert_eq!(s[0].to_rfc3339(), "2026-10-30T11:00:00+00:00");
        assert_eq!(s[1].to_rfc3339(), "2026-10-30T23:00:00+00:00");
        let taken = [s[0]];
        assert_eq!(next_slots(&taken, 2, 1, now)[0], s[1]);
    }

    #[test]
    fn links_accept_handles_and_urls_and_skip_blanks() {
        let s = Socials { kick: "frijolito-trash".into(), tiktok: "@me".into(), instagram: "https://instagram.com/me".into(), ..Default::default() };
        assert_eq!(links_text(&s), "Follow me:\nKick: https://kick.com/frijolito-trash\nTikTok: https://www.tiktok.com/@me\nInstagram: https://instagram.com/me");
        assert_eq!(links_text(&Socials::default()), "");
    }

    #[test]
    fn descriptions_end_with_the_links() {
        let db = Db::memory().unwrap();
        let mut s = Settings::load(&db);
        s.socials.instagram = "frijolito".into();
        s.kick_channel_slug = Some("frijolito-trash".into());
        s.save(&db).unwrap();
        let d = with_links(&db, "Full stream.");
        assert!(d.starts_with("Full stream.\n\nFollow me:"), "{d}");
        assert!(d.contains("https://kick.com/frijolito-trash") && d.contains("https://www.instagram.com/frijolito"));
    }

    #[test]
    fn auto_upload_only_touches_streams_marked_for_it() {
        let db = Db::memory().unwrap();
        db.conn().execute("INSERT INTO streams(source,title,status,duration_s) VALUES('local','old','analyzed',100)", []).unwrap();
        db.conn().execute("INSERT INTO streams(source,title,status,duration_s,auto_upload) VALUES('kick','new','analyzed',100,1)", []).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(auto_upload(&db, 1)).unwrap();
        assert!(list_posts(&db).unwrap().is_empty());
        rt.block_on(auto_upload(&db, 2)).unwrap();
        rt.block_on(auto_upload(&db, 2)).unwrap();
        let p = list_posts(&db).unwrap();
        assert_eq!(p.len(), 1);
        assert_eq!((p[0].status.as_str(), p[0].privacy.as_str()), ("scheduled", "private"));
    }

    #[test]
    fn hashtags_from_the_game_and_settings_without_repeats() {
        assert_eq!(hashtag("Project Zomboid"), "#ProjectZomboid");
        assert_eq!(hashtag("#kick"), "#Kick");
        let line = hashtag_line(Some("Project Zomboid"), &["#zombies".into(), "#ProjectZomboid".into()], &["#kick".into(), "livestream".into()]);
        assert_eq!(line, "#ProjectZomboid #Zombies #Kick #Livestream");
        assert_eq!(hashtag_line(None, &[], &[]), "");
    }

    #[test]
    fn full_stream_description_has_links_then_hashtags() {
        let db = Db::memory().unwrap();
        db.conn().execute("INSERT INTO streams(source,title,status,duration_s,category) VALUES('kick','Love story','analyzed',100,'Project Zomboid')", []).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let id = rt.block_on(draft_full_video(&db, 1)).unwrap();
        let d = get_post(&db, id).unwrap().description;
        assert!(d.starts_with("Full stream of Project Zomboid"), "{d}");
        assert!(d.trim_end().ends_with("#ProjectZomboid #Kick #Livestream #Gaming"), "{d}");
    }

    #[test]
    fn editing_a_published_youtube_post_queues_an_update() {
        let db = Db::memory().unwrap();
        db.conn().execute("INSERT INTO streams(source,title,status) VALUES('local','x','analyzed')", []).unwrap();
        db.conn().execute("INSERT INTO posts(stream_id,platform,title,status,remote_id) VALUES(1,'youtube','t','published','abc')", []).unwrap();
        update_post(&db, 1, "New title", "d", &[], "public").unwrap();
        let jobs = db.list_jobs(10).unwrap();
        assert_eq!((jobs[0].kind.as_str(), jobs[0].payload.as_str()), ("update_video", "{\"post\":1}"));
    }

    #[test]
    fn manual_clips_are_approved_and_queued_for_render() {
        let db = Db::memory().unwrap();
        db.conn().execute("INSERT INTO streams(source,title,status,duration_s) VALUES('local','x','analyzed',600)", []).unwrap();
        db.conn().execute("INSERT INTO transcripts(stream_id,words,engine) VALUES(1,'[[100.0,100.4,\"hola\"],[101.0,101.3,\"n****\"],[300.0,300.3,\"later\"]]',2)", []).unwrap();
        let id = crate::studio::create_manual_clip(&db, 1, 95.0, 125.0, "").unwrap();
        let (status, render, by, words): (String, String, String, String) = db.conn()
            .query_row("SELECT status,render_status,scored_by,words FROM clips WHERE id=?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap();
        assert_eq!((status.as_str(), render.as_str(), by.as_str()), ("approved", "queued", "manual"));
        assert!(words.contains("\"b\":true"), "the censored word stays bleeped: {words}");
        assert!(crate::studio::create_manual_clip(&db, 1, 0.0, 2.0, "").is_err(), "too short");
        assert!(crate::studio::create_manual_clip(&db, 1, 550.0, 650.0, "").is_err(), "past the end");
    }

    #[test]
    fn duration_limits() {
        assert!(duration_problem("youtube", 120.0).is_none());
        assert!(duration_problem("youtube", 200.0).is_some());
        assert!(duration_problem("facebook", 95.0).is_some());
        assert!(duration_problem("tiktok", 120.0).is_none());
    }

    #[test]
    fn schedule_queues_a_delayed_publish_job_and_unschedule_removes_it() {
        let db = Db::memory().unwrap();
        db.conn().execute("INSERT INTO streams(source,title,status) VALUES('local','x','probed')", []).unwrap();
        db.conn().execute("INSERT INTO posts(stream_id,platform,title) VALUES(1,'youtube','t')", []).unwrap();
        schedule(&db, 1, Some("2030-01-01T10:00:00Z")).unwrap();
        let j = &db.list_jobs(10).unwrap()[0];
        assert_eq!((j.kind.as_str(), j.run_after.as_str()), ("publish", "2030-01-01T10:00:00Z"));
        assert!(db.claim_next().unwrap().is_none(), "not due yet");
        unschedule(&db, 1).unwrap();
        assert!(db.list_jobs(10).unwrap().is_empty());
        assert_eq!(get_post(&db, 1).unwrap().status, "draft");
    }
}
