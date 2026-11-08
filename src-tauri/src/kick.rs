//! Kick stream monitor, using the official public API (docs.kick.com).
//!
//! - Auth: App Access Token via `client_credentials` at id.kick.com (public data only).
//! - Live detection: polls `GET /public/v1/channels?slug=<slug>` (`stream.is_live`).
//!   Webhooks (`livestream.status.updated`) need an internet-reachable endpoint, which a
//!   desktop app does not have, so polling is used instead.
//! - VODs: the public API has no VOD endpoint, so past broadcasts come from the website's own
//!   API (`kick.com/api`). It's unofficial and can change without notice, so a failure here
//!   is reported and never touches recordings already on disk. A local recording, when one
//!   exists, is matched to its session in `ingest.rs` and takes priority: Kick keeps VODs at
//!   a lower quality than a recording.

use crate::{db::Db, jobs::now_str, secrets, settings::Settings};
use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, NaiveDateTime, Utc};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const TOKEN_URL: &str = "https://id.kick.com/oauth/token";
const CHANNELS_URL: &str = "https://api.kick.com/public/v1/channels";
const SITE_API: &str = "https://kick.com/api";
/// Named as what it is. Kick's site API answers 403 to clients claiming to be a browser
/// without behaving like one.
const USER_AGENT: &str = concat!("StreamForgeAI/", env!("CARGO_PKG_VERSION"));
/// How far apart a VOD's start and a session's start can be and still be the same stream.
const VOD_MATCH_SECS: i64 = 600;
/// How long to keep looking for a stream's VOD after it ends.
const VOD_WAIT_HOURS: i64 = 12;

#[derive(Debug, Clone, Default, Serialize)]
pub struct KickState {
    pub configured: bool,
    pub live: bool,
    pub slug: Option<String>,
    pub title: Option<String>,
    pub started_at: Option<String>,
    pub viewers: Option<i64>,
    pub category: Option<String>,
    pub last_checked: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, PartialEq, Deserialize)]
pub struct ChannelStatus {
    pub slug: String,
    pub is_live: bool,
    pub title: String,
    pub started_at: Option<String>,
    pub viewers: Option<i64>,
    pub category: Option<String>,
}

pub fn parse_channel(v: &Value) -> Result<ChannelStatus> {
    let ch = v["data"].get(0).ok_or_else(|| anyhow!("Kick returned no channel for that slug"))?;
    let stream = &ch["stream"];
    Ok(ChannelStatus {
        slug: ch["slug"].as_str().unwrap_or_default().to_string(),
        is_live: stream["is_live"].as_bool().unwrap_or(false),
        title: ch["stream_title"].as_str().unwrap_or_default().to_string(),
        started_at: stream["start_time"].as_str().filter(|s| !s.is_empty() && !s.starts_with("0001")).map(String::from),
        viewers: stream["viewer_count"].as_i64(),
        category: ch["category"]["name"].as_str().map(String::from),
    })
}

/// One past broadcast on Kick.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Vod {
    pub id: String,
    pub title: String,
    /// RFC 3339, UTC.
    pub started_at: String,
    pub duration_s: f64,
    /// HLS playlist.
    pub source: String,
    pub thumbnail: Option<String>,
    /// The game or category streamed, e.g. "Project Zomboid".
    #[serde(default)]
    pub category: Option<String>,
}

/// Kick writes both `2026-11-06 01:47:03` and `2026-11-06T01:47:03+00:00`.
fn parse_time(t: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(t)
        .map(|d| d.with_timezone(&Utc))
        .ok()
        .or_else(|| NaiveDateTime::parse_from_str(t, "%Y-%m-%d %H:%M:%S").ok().map(|d| d.and_utc()))
}

fn parse_vod(live: &Value, source: Option<&str>) -> Option<Vod> {
    if live["is_live"].as_bool().unwrap_or(false) {
        return None;
    }
    let id = live["id"].as_i64().map(|i| i.to_string()).or_else(|| live["id"].as_str().map(String::from))?;
    let started = parse_time(live["start_time"].as_str()?)?;
    let source = source.or(live["source"].as_str()).filter(|s| !s.is_empty())?.to_string();
    Some(Vod {
        id,
        title: live["session_title"].as_str().filter(|t| !t.is_empty()).unwrap_or("Kick stream").to_string(),
        started_at: started.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        duration_s: live["duration"].as_f64().unwrap_or(0.0) / 1000.0,
        source,
        thumbnail: live["thumbnail"]["src"].as_str().or(live["thumbnail"].as_str()).map(String::from),
        category: live["categories"][0]["name"].as_str().map(String::from),
    })
}

/// Parses `/api/v2/channels/<slug>/videos`.
pub fn parse_vods(v: &Value) -> Vec<Vod> {
    v.as_array().map(|a| a.iter().filter_map(|x| parse_vod(x, None)).collect()).unwrap_or_default()
}

/// Parses `/api/v1/video/<uuid>`.
pub fn parse_video(v: &Value) -> Option<Vod> {
    parse_vod(&v["livestream"], v["source"].as_str())
}

/// The video id in a link like `kick.com/channel/videos/<uuid>`, or a bare id.
pub fn video_uuid(input: &str) -> Option<String> {
    let last = input.trim().split(['?', '#']).next()?.trim_end_matches('/').rsplit('/').next()?;
    let parts: Vec<&str> = last.split('-').collect();
    let ok = [8, 4, 4, 4, 12].iter().zip(&parts).all(|(n, p)| p.len() == *n && p.chars().all(|c| c.is_ascii_hexdigit()));
    (parts.len() == 5 && ok).then(|| last.to_lowercase())
}

/// The VOD for a session that started at `started`, if Kick has one yet.
pub fn match_vod<'a>(vods: &'a [Vod], started: &str) -> Option<&'a Vod> {
    let t = parse_time(started)?;
    vods.iter()
        .filter_map(|v| Some((v, (parse_time(&v.started_at)? - t).num_seconds().abs())))
        .filter(|(_, gap)| *gap <= VOD_MATCH_SECS)
        .min_by_key(|(_, gap)| *gap)
        .map(|(v, _)| v)
}

async fn site_json(path: &str) -> Result<Value> {
    let resp = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent(USER_AGENT)
        .build()?
        .get(format!("{SITE_API}{path}"))
        .header("Accept", "application/json")
        .send()
        .await
        .context("could not reach kick.com")?;
    let status = resp.status();
    if !status.is_success() {
        return Err(anyhow!("Kick refused the VOD request ({status})"));
    }
    Ok(resp.json().await?)
}

pub async fn list_vods(slug: &str) -> Result<Vec<Vod>> {
    Ok(parse_vods(&site_json(&format!("/v2/channels/{slug}/videos")).await?))
}

/// Looks up one VOD from a pasted link.
pub async fn vod_from_link(link: &str) -> Result<Vod> {
    let id = video_uuid(link).ok_or_else(|| anyhow!("That doesn't look like a Kick video link"))?;
    parse_video(&site_json(&format!("/v1/video/{id}")).await?).ok_or_else(|| anyhow!("Kick didn't return a playable video for that link"))
}

/// Adds a VOD to the library and queues its download. Already-known VODs are left alone.
pub fn add_vod(db: &Db, v: &Vod) -> Result<i64> {
    if let Some(id) = db.conn().query_row("SELECT id FROM streams WHERE kick_vod_id=?1", [&v.id], |r| r.get(0)).optional()? {
        return Ok(id);
    }
    let id = {
        let conn = db.conn();
        conn.execute(
            "INSERT INTO streams(source,title,kick_started,kick_vod_id,vod_source,status,category) VALUES('kick',?1,?2,?3,?4,'downloading',?5)",
            rusqlite::params![v.title, v.started_at, v.id, v.source, v.category],
        )?;
        conn.last_insert_rowid()
    };
    db.enqueue("download_vod", Some(id), &duration_payload(v.duration_s))?;
    Ok(id)
}

fn duration_payload(duration_s: f64) -> String {
    format!("{{\"duration\":{duration_s}}}")
}

/// After a stream ends: use the local recording if one arrived, otherwise fetch the VOD.
/// Returns the job message. Asks to be called again while Kick hasn't published the VOD.
pub async fn find_vod(db: &Db, sid: i64, attempt: i64, slug: &str) -> Result<String> {
    let s = db.get_stream(sid)?.ok_or_else(|| anyhow!("stream #{sid} no longer exists"))?;
    if s.status != "awaiting_file" {
        return Ok("Recording already found".into());
    }
    if let Some(id) = crate::ingest::adopt_recording(db, sid)? {
        return Ok(format!("Matched to the recording already in the library (stream {id})"));
    }
    let started = s.kick_started.clone().ok_or_else(|| anyhow!("stream has no start time"))?;
    let vods = list_vods(slug).await?;
    match match_vod(&vods, &started) {
        Some(v) => {
            db.conn().execute(
                "UPDATE streams SET kick_vod_id=?2, vod_source=?3, status='downloading', category=COALESCE(category, ?4) WHERE id=?1",
                rusqlite::params![sid, v.id, v.source, v.category],
            )?;
            db.enqueue("download_vod", Some(sid), &duration_payload(v.duration_s))?;
            Ok(format!("Found the Kick VOD for {}", v.title))
        }
        None if attempt * 10 < VOD_WAIT_HOURS * 60 => {
            let at = (Utc::now() + chrono::Duration::minutes(10)).format("%Y-%m-%dT%H:%M:%SZ").to_string();
            db.enqueue_at("find_vod", Some(sid), &format!("{{\"try\":{}}}", attempt + 1), &at)?;
            Ok("Kick hasn't published the VOD yet; checking again in 10 minutes".into())
        }
        None => Err(anyhow!("Kick never published a VOD for this stream")),
    }
}

/// Reserved characters out of a stream title, for a file name.
pub fn file_title(title: &str) -> String {
    let t: String = title.chars().map(|c| if c.is_alphanumeric() || " -_".contains(c) { c } else { ' ' }).collect();
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    t.chars().take(60).collect::<String>().trim().to_string()
}

pub struct KickClient {
    http: reqwest::Client,
    token: Mutex<Option<(String, Instant)>>,
}

impl KickClient {
    pub fn new() -> Self {
        KickClient {
            http: reqwest::Client::builder().timeout(Duration::from_secs(20)).build().expect("http client"),
            token: Mutex::new(None),
        }
    }

    pub fn forget_token(&self) {
        *self.token.lock().unwrap() = None;
    }

    async fn app_token(&self) -> Result<String> {
        if let Some((t, exp)) = self.token.lock().unwrap().clone() {
            if Instant::now() < exp {
                return Ok(t);
            }
        }
        let id = secrets::get("kick.client_id")?.ok_or_else(|| anyhow!("Kick Client ID not set"))?;
        let secret = secrets::get("kick.client_secret")?.ok_or_else(|| anyhow!("Kick Client Secret not set"))?;
        let resp = self
            .http
            .post(TOKEN_URL)
            .form(&[("grant_type", "client_credentials"), ("client_id", &id), ("client_secret", &secret)])
            .send()
            .await
            .context("could not reach id.kick.com")?;
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            // Body may echo request details; report only the status and Kick's message.
            let msg = body["message"].as_str().or(body["error_description"].as_str()).unwrap_or("");
            return Err(anyhow!("Kick token request failed ({status}) {msg}"));
        }
        let token = body["access_token"].as_str().ok_or_else(|| anyhow!("Kick token response had no access_token"))?.to_string();
        let ttl = body["expires_in"].as_u64().unwrap_or(3600).saturating_sub(60).max(60);
        *self.token.lock().unwrap() = Some((token.clone(), Instant::now() + Duration::from_secs(ttl)));
        Ok(token)
    }

    pub async fn channel(&self, slug: &str) -> Result<ChannelStatus> {
        let token = self.app_token().await?;
        let resp = self
            .http
            .get(CHANNELS_URL)
            .query(&[("slug", slug)])
            .bearer_auth(token)
            .send()
            .await
            .context("could not reach api.kick.com")?;
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            self.forget_token(); // expired or revoked; next poll fetches a new one
        }
        let status = resp.status();
        if !status.is_success() {
            return Err(anyhow!("Kick channels request failed ({status})"));
        }
        parse_channel(&resp.json().await?)
    }
}

/// What a poll result means for the stream record.
#[derive(Debug, PartialEq)]
pub enum Transition {
    Started,
    Ended,
    None,
}

pub fn transition(was_live: bool, is_live: bool) -> Transition {
    match (was_live, is_live) {
        (false, true) => Transition::Started,
        (true, false) => Transition::Ended,
        _ => Transition::None,
    }
}

const LIVE_ROW_KEY: &str = "kick_live_stream_id";

/// Records start/end of a Kick session as a `streams` row.
fn apply(db: &Db, t: Transition, ch: &ChannelStatus) -> Result<()> {
    match t {
        Transition::Started => {
            let title = if ch.title.is_empty() { "Kick stream".to_string() } else { ch.title.clone() };
            let auto = Settings::load(db).auto_upload_vod;
            let conn = db.conn();
            conn.execute(
                "INSERT INTO streams(source,title,kick_started,status,auto_upload,category) VALUES('kick',?1,?2,'live',?3,?4)",
                rusqlite::params![title, ch.started_at.clone().unwrap_or_else(now_str), auto as i64, ch.category],
            )?;
            let id = conn.last_insert_rowid();
            drop(conn);
            db.set_setting(LIVE_ROW_KEY, &id.to_string())?;
            db.log("info", "kick", &format!("Went live: {title}"));
        }
        Transition::Ended => {
            if let Some(id) = db.get_setting(LIVE_ROW_KEY)?.and_then(|s| s.parse::<i64>().ok()) {
                // A recording may already be attached (OBS finished before Kick said offline).
                db.conn().execute(
                    "UPDATE streams SET kick_ended=?2, status=CASE status WHEN 'live' THEN 'awaiting_file' ELSE status END WHERE id=?1",
                    rusqlite::params![id, now_str()],
                )?;
                // Gives a local recording time to finish writing and be picked up first.
                let at = (Utc::now() + chrono::Duration::minutes(10)).format("%Y-%m-%dT%H:%M:%SZ").to_string();
                db.enqueue_at("find_vod", Some(id), "{\"try\":1}", &at)?;
            }
            db.set_setting(LIVE_ROW_KEY, "")?;
            db.log("info", "kick", "Stream ended; looking for its recording, then the Kick VOD");
        }
        Transition::None => {}
    }
    Ok(())
}

/// Whether the DB says we are mid-stream (survives app restarts).
fn db_thinks_live(db: &Db) -> bool {
    db.get_setting(LIVE_ROW_KEY).ok().flatten().is_some_and(|s| !s.is_empty())
}

pub async fn monitor_loop(db: Arc<Db>, client: Arc<KickClient>, state: Arc<Mutex<KickState>>) {
    loop {
        let settings = Settings::load(&db);
        let configured = settings.kick_channel_slug.is_some()
            && secrets::is_set("kick.client_id")
            && secrets::is_set("kick.client_secret");
        state.lock().unwrap().configured = configured;

        if configured {
            let slug = settings.kick_channel_slug.clone().unwrap();
            let result = client.channel(&slug).await;
            let mut st = state.lock().unwrap();
            st.slug = Some(slug);
            st.last_checked = Some(now_str());
            match result {
                Ok(ch) => {
                    let t = transition(db_thinks_live(&db), ch.is_live);
                    if let Err(e) = apply(&db, t, &ch) {
                        db.log("error", "kick", &format!("Could not record stream status: {e:#}"));
                    }
                    st.live = ch.is_live;
                    st.title = Some(ch.title);
                    st.started_at = ch.started_at;
                    st.viewers = ch.viewers;
                    st.category = ch.category;
                    st.last_error = None;
                }
                Err(e) => {
                    let msg = format!("{e:#}");
                    if st.last_error.as_deref() != Some(&msg) {
                        db.log("warn", "kick", &msg);
                    }
                    st.last_error = Some(msg);
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(settings.kick_poll_secs.max(30))).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample(live: bool) -> Value {
        json!({"data": [{
            "broadcaster_user_id": 1, "slug": "me", "stream_title": "Ranked grind",
            "category": {"id": 5, "name": "Just Chatting"},
            "stream": {"is_live": live, "start_time": "2026-10-29T01:00:00Z", "viewer_count": 42}
        }], "message": "OK"})
    }

    fn vod_json() -> Value {
        json!([
            {"id": 136250421, "session_title": "Love story 2", "start_time": "2026-11-06 01:47:03", "duration": 14785000, "is_live": false,
             "source": "https://stream.kick.com/a/master.m3u8", "thumbnail": {"src": "https://x/t.webp"}, "categories": [{"name": "Project Zomboid"}]},
            {"id": 3, "session_title": "Now", "start_time": "2026-11-07 01:00:00", "duration": 0, "is_live": true, "source": "https://x/l.m3u8"}
        ])
    }

    #[test]
    fn parses_vod_list_and_skips_live_sessions() {
        let v = parse_vods(&vod_json());
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].id, "136250421");
        assert_eq!(v[0].started_at, "2026-11-06T01:47:03Z");
        assert_eq!(v[0].duration_s, 14785.0);
        assert_eq!(v[0].thumbnail.as_deref(), Some("https://x/t.webp"));
        assert_eq!(v[0].category.as_deref(), Some("Project Zomboid"));
    }

    #[test]
    fn parses_single_video_shape() {
        let j = json!({"uuid": "u", "source": "https://x/m.m3u8", "livestream": {"id": 9, "session_title": "T", "start_time": "2026-11-06T01:47:03+00:00", "duration": 1000, "is_live": false, "thumbnail": "https://x/t"}});
        let v = parse_video(&j).unwrap();
        assert_eq!((v.id.as_str(), v.source.as_str(), v.duration_s), ("9", "https://x/m.m3u8", 1.0));
    }

    #[test]
    fn finds_video_id_in_links() {
        let id = "3b424fd3-557e-460e-9605-953777d21f2e";
        assert_eq!(video_uuid(&format!("https://kick.com/frijolito-trash/videos/{id}?t=1")).as_deref(), Some(id));
        assert_eq!(video_uuid(id).as_deref(), Some(id));
        assert_eq!(video_uuid("https://kick.com/frijolito-trash"), None);
    }

    #[test]
    fn vod_matches_only_the_session_it_belongs_to() {
        let v = parse_vods(&vod_json());
        assert!(match_vod(&v, "2026-11-06T01:47:00Z").is_some());
        assert!(match_vod(&v, "2026-11-05T23:00:27Z").is_none(), "an earlier stream the same night");
    }

    #[test]
    fn vods_are_added_once() {
        let db = Db::memory().unwrap();
        let v = parse_vods(&vod_json()).remove(0);
        let id = add_vod(&db, &v).unwrap();
        assert_eq!(add_vod(&db, &v).unwrap(), id);
        assert_eq!(db.get_stream(id).unwrap().unwrap().status, "downloading");
        assert_eq!(db.job_counts().unwrap().queued, 1);
    }

    #[test]
    fn titles_become_safe_file_names() {
        assert_eq!(file_title("BEST: player?  #2 / <final>"), "BEST player 2 final");
    }

    /// Live call: `cargo test real_kick -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn real_kick_vod_list() {
        let v = list_vods("frijolito-trash").await.unwrap();
        assert!(!v.is_empty());
        println!("{v:#?}");
        let one = vod_from_link("3b424fd3-557e-460e-9605-953777d21f2e").await.unwrap();
        assert!(one.source.ends_with(".m3u8"), "{}", one.source);
    }

    #[test]
    fn parses_documented_channel_shape() {
        let c = parse_channel(&sample(true)).unwrap();
        assert!(c.is_live);
        assert_eq!(c.title, "Ranked grind");
        assert_eq!(c.category.as_deref(), Some("Just Chatting"));
        assert_eq!(c.viewers, Some(42));
    }

    #[test]
    fn empty_data_is_an_error() {
        assert!(parse_channel(&json!({"data": []})).is_err());
    }

    #[test]
    fn live_session_is_recorded_and_closed() {
        let db = Db::memory().unwrap();
        let live = parse_channel(&sample(true)).unwrap();
        apply(&db, transition(db_thinks_live(&db), true), &live).unwrap();
        assert!(db_thinks_live(&db));
        assert_eq!(transition(db_thinks_live(&db), true), Transition::None);
        apply(&db, transition(db_thinks_live(&db), false), &live).unwrap();
        let s = &db.list_streams().unwrap()[0];
        assert_eq!(s.status, "awaiting_file");
        assert!(s.kick_ended.is_some());
        assert!(!db_thinks_live(&db));
    }
}
