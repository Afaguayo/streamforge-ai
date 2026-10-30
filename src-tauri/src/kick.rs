//! Kick stream monitor, using the official public API (docs.kick.com).
//!
//! - Auth: App Access Token via `client_credentials` at id.kick.com (public data only).
//! - Live detection: polls `GET /public/v1/channels?slug=<slug>` (`stream.is_live`).
//!   Webhooks (`livestream.status.updated`) need an internet-reachable endpoint, which a
//!   desktop app does not have, so polling is used instead.
//! - VODs: the public API has no VOD/past-broadcast endpoint. Recordings come from the
//!   local recording folder (see `ingest.rs`) and are matched to the Kick session there.

use crate::{db::Db, jobs::now_str, secrets, settings::Settings};
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const TOKEN_URL: &str = "https://id.kick.com/oauth/token";
const CHANNELS_URL: &str = "https://api.kick.com/public/v1/channels";

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
            let conn = db.conn();
            conn.execute(
                "INSERT INTO streams(source,title,kick_started,status) VALUES('kick',?1,?2,'live')",
                rusqlite::params![title, ch.started_at.clone().unwrap_or_else(now_str)],
            )?;
            let id = conn.last_insert_rowid();
            drop(conn);
            db.set_setting(LIVE_ROW_KEY, &id.to_string())?;
            db.log("info", "kick", &format!("Went live: {title}"));
        }
        Transition::Ended => {
            if let Some(id) = db.get_setting(LIVE_ROW_KEY)?.and_then(|s| s.parse::<i64>().ok()) {
                db.conn().execute(
                    "UPDATE streams SET kick_ended=?2, status='awaiting_file' WHERE id=?1 AND status='live'",
                    rusqlite::params![id, now_str()],
                )?;
            }
            db.set_setting(LIVE_ROW_KEY, "")?;
            db.log("info", "kick", "Stream ended; waiting for the recording to appear in the recording folder");
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
