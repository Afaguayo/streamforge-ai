//! One-click sign-in for Kick, YouTube, TikTok and Meta (Facebook Pages + Instagram).
//!
//! Flow: open the platform's consent page in the default browser, catch the redirect on a
//! local loopback port, exchange the code for tokens, store them in Windows Credential
//! Manager and the account name in SQLite. Tokens refresh automatically when used.
//!
//! Each platform needs a developer app the user owns (client ID + secret). A public build
//! can't ship shared secrets, and app review/audit is tied to the app owner.

use crate::{db::Db, jobs::now_str, secrets, settings::Settings};
use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::{distributions::Alphanumeric, Rng};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub const REDIRECT_PORT: u16 = 47821;
const META_GRAPH: &str = "https://graph.facebook.com/v26.0";
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);

pub fn redirect_uri() -> String {
    format!("http://localhost:{REDIRECT_PORT}/callback")
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Pkce {
    /// RFC 7636: base64url(SHA-256(verifier)).
    S256,
    /// TikTok desktop: hex(SHA-256(verifier)), still sent as method S256.
    S256Hex,
    None,
}

pub struct Provider {
    pub id: &'static str,
    pub name: &'static str,
    auth_url: &'static str,
    token_url: &'static str,
    scopes: &'static [&'static str],
    scope_sep: &'static str,
    /// Credential names for the developer app.
    pub id_key: &'static str,
    pub secret_key: &'static str,
    /// Query parameter name for the client ID.
    id_param: &'static str,
    pkce: Pkce,
    extra_auth: &'static [(&'static str, &'static str)],
}

impl Provider {
    fn token_key(&self) -> String {
        format!("{}.token", self.id)
    }
}

pub const PROVIDERS: &[Provider] = &[
    Provider {
        id: "kick",
        name: "Kick",
        auth_url: "https://id.kick.com/oauth/authorize",
        token_url: "https://id.kick.com/oauth/token",
        scopes: &["user:read", "channel:read"],
        scope_sep: " ",
        id_key: "kick.client_id",
        secret_key: "kick.client_secret",
        id_param: "client_id",
        pkce: Pkce::S256,
        extra_auth: &[],
    },
    Provider {
        id: "youtube",
        name: "YouTube",
        auth_url: "https://accounts.google.com/o/oauth2/v2/auth",
        token_url: "https://oauth2.googleapis.com/token",
        scopes: &[
            "https://www.googleapis.com/auth/youtube.upload",
            "https://www.googleapis.com/auth/youtube.readonly",
            "https://www.googleapis.com/auth/yt-analytics.readonly",
        ],
        scope_sep: " ",
        id_key: "youtube.client_id",
        secret_key: "youtube.client_secret",
        id_param: "client_id",
        pkce: Pkce::S256,
        // offline + consent so Google always returns a refresh token.
        extra_auth: &[("access_type", "offline"), ("prompt", "consent")],
    },
    Provider {
        id: "tiktok",
        name: "TikTok",
        auth_url: "https://www.tiktok.com/v2/auth/authorize/",
        token_url: "https://open.tiktokapis.com/v2/oauth/token/",
        scopes: &["user.info.basic", "video.upload", "video.publish"],
        scope_sep: ",",
        id_key: "tiktok.client_key",
        secret_key: "tiktok.client_secret",
        id_param: "client_key",
        pkce: Pkce::S256Hex,
        extra_auth: &[],
    },
    Provider {
        id: "meta",
        name: "Facebook + Instagram",
        auth_url: "https://www.facebook.com/v26.0/dialog/oauth",
        token_url: "https://graph.facebook.com/v26.0/oauth/access_token",
        scopes: &[
            "pages_show_list",
            "pages_read_engagement",
            "pages_manage_posts",
            "business_management",
            "instagram_basic",
            "instagram_content_publish",
        ],
        scope_sep: ",",
        id_key: "meta.app_id",
        secret_key: "meta.app_secret",
        id_param: "client_id",
        pkce: Pkce::None,
        extra_auth: &[],
    },
];

pub fn provider(id: &str) -> Result<&'static Provider> {
    PROVIDERS.iter().find(|p| p.id == id).ok_or_else(|| anyhow!("unknown platform '{id}'"))
}

// ---------- PKCE / URL helpers ----------

fn random_string(len: usize) -> String {
    rand::thread_rng().sample_iter(&Alphanumeric).take(len).map(char::from).collect()
}

pub fn code_challenge(verifier: &str, pkce: Pkce) -> Option<String> {
    let digest = Sha256::digest(verifier.as_bytes());
    match pkce {
        Pkce::S256 => Some(URL_SAFE_NO_PAD.encode(digest)),
        Pkce::S256Hex => Some(digest.iter().map(|b| format!("{b:02x}")).collect()),
        Pkce::None => None,
    }
}

pub fn auth_url(p: &Provider, client_id: &str, state: &str, verifier: &str) -> String {
    let mut url = Url::parse(p.auth_url).expect("static url");
    {
        let mut q = url.query_pairs_mut();
        q.append_pair(p.id_param, client_id)
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", &redirect_uri())
            .append_pair("scope", &p.scopes.join(p.scope_sep))
            .append_pair("state", state);
        if let Some(c) = code_challenge(verifier, p.pkce) {
            q.append_pair("code_challenge", &c).append_pair("code_challenge_method", "S256");
        }
        for (k, v) in p.extra_auth {
            q.append_pair(k, v);
        }
    }
    url.into()
}

#[derive(Debug, PartialEq)]
pub enum Callback {
    Code(String),
    Denied(String),
    /// Not the OAuth callback (e.g. /favicon.ico); keep waiting.
    Other,
}

/// Parses the request line of the browser's redirect, checking `state`.
pub fn parse_callback(request: &str, expected_state: &str) -> Result<Callback> {
    let target = request.split_whitespace().nth(1).unwrap_or("/");
    let url = Url::parse(&format!("http://localhost{target}"))?;
    if url.path().trim_end_matches('/') != "/callback" {
        return Ok(Callback::Other);
    }
    let get = |k: &str| url.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.to_string());
    if let Some(err) = get("error") {
        let desc = get("error_description").or(get("error_reason")).unwrap_or_default();
        return Ok(Callback::Denied(format!("{err} {desc}").trim().to_string()));
    }
    if get("state").as_deref() != Some(expected_state) {
        bail!("sign-in response didn't match this request (state mismatch); try again");
    }
    get("code").map(Callback::Code).ok_or_else(|| anyhow!("redirect had no authorization code"))
}

const DONE_PAGE: &str = "<!doctype html><meta charset=utf-8><title>StreamForge AI</title>\
<body style=\"font-family:Segoe UI,sans-serif;background:#0d0f14;color:#e6e9ef;display:grid;place-items:center;height:100vh;margin:0\">\
<div style=\"text-align:center\"><h2>{TITLE}</h2><p style=\"color:#a3abba\">{BODY}</p></div>";

async fn respond(stream: &mut tokio::net::TcpStream, title: &str, body: &str) {
    let html = DONE_PAGE.replace("{TITLE}", title).replace("{BODY}", body);
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
        html.len()
    );
    let _ = stream.write_all(resp.as_bytes()).await;
    let _ = stream.shutdown().await;
}

/// Waits on both IPv4 and IPv6 loopback, since browsers may resolve `localhost` to either.
async fn wait_for_code(state: &str) -> Result<String> {
    let v4 = TcpListener::bind(("127.0.0.1", REDIRECT_PORT))
        .await
        .with_context(|| format!("port {REDIRECT_PORT} is busy; is another sign-in still open?"))?;
    let v6 = TcpListener::bind(("::1", REDIRECT_PORT)).await.ok();
    loop {
        let accepted = match &v6 {
            Some(v6) => tokio::select! { a = v4.accept() => a, a = v6.accept() => a },
            None => v4.accept().await,
        };
        let (mut stream, _) = accepted?;
        let mut buf = vec![0u8; 8192];
        let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf)).await.unwrap_or(Ok(0)).unwrap_or(0);
        let req = String::from_utf8_lossy(&buf[..n]).to_string();
        match parse_callback(req.lines().next().unwrap_or(""), state) {
            Ok(Callback::Code(code)) => {
                respond(&mut stream, "Signed in ✓", "You can close this tab and go back to StreamForge AI.").await;
                return Ok(code);
            }
            Ok(Callback::Denied(why)) => {
                respond(&mut stream, "Sign-in cancelled", &html_escape(&why)).await;
                bail!("sign-in was declined: {why}");
            }
            Ok(Callback::Other) => respond(&mut stream, "", "").await,
            Err(e) => {
                respond(&mut stream, "Sign-in failed", &html_escape(&e.to_string())).await;
                return Err(e);
            }
        }
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

// ---------- tokens ----------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TokenSet {
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// Unix seconds.
    pub expires_at: Option<i64>,
    pub refresh_expires_at: Option<i64>,
}

fn unix_now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Reads a token response (Google/Kick/TikTok/Meta all use these names).
/// `previous_refresh` is kept when a refresh response omits a new refresh token.
pub fn parse_token(v: &Value, previous_refresh: Option<String>) -> Result<TokenSet> {
    if let Some(err) = v["error"].as_str() {
        bail!("token request rejected: {}", v["error_description"].as_str().unwrap_or(err));
    }
    let access = v["access_token"].as_str().ok_or_else(|| anyhow!("token response had no access_token"))?;
    let now = unix_now();
    Ok(TokenSet {
        access_token: access.to_string(),
        refresh_token: v["refresh_token"].as_str().map(String::from).or(previous_refresh),
        expires_at: v["expires_in"].as_i64().map(|s| now + s),
        refresh_expires_at: v["refresh_expires_in"].as_i64().map(|s| now + s),
    })
}

fn load_tokens(p: &Provider) -> Result<Option<TokenSet>> {
    Ok(match secrets::get(&p.token_key())? {
        Some(s) => Some(serde_json::from_str(&s).context("stored token is corrupt; sign in again")?),
        None => None,
    })
}

fn store_tokens(p: &Provider, t: &TokenSet) -> Result<()> {
    secrets::set(&p.token_key(), &serde_json::to_string(t)?)
}

fn app_keys(p: &Provider) -> Result<(String, String)> {
    let id = secrets::get(p.id_key)?.ok_or_else(|| anyhow!("Add your {} app's client ID first", p.name))?;
    let secret = secrets::get(p.secret_key)?.ok_or_else(|| anyhow!("Add your {} app's client secret first", p.name))?;
    Ok((id, secret))
}

fn http() -> reqwest::Client {
    reqwest::Client::builder().timeout(Duration::from_secs(30)).build().expect("http client")
}

async fn json_of(resp: reqwest::Response) -> Result<Value> {
    let status = resp.status();
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        let msg = v["error_description"]
            .as_str()
            .or(v["error"]["message"].as_str())
            .or(v["message"].as_str())
            .or(v["error"].as_str())
            .unwrap_or("");
        bail!("HTTP {status} {msg}");
    }
    Ok(v)
}

async fn exchange_code(p: &Provider, code: &str, verifier: &str) -> Result<TokenSet> {
    let (id, secret) = app_keys(p)?;
    let c = http();
    if p.id == "meta" {
        let short = json_of(
            c.get(p.token_url)
                .query(&[("client_id", id.as_str()), ("client_secret", &secret), ("redirect_uri", &redirect_uri()), ("code", code)])
                .send()
                .await?,
        )
        .await?;
        let short = parse_token(&short, None)?;
        // Swap the 1–2 h token for a ~60 day one.
        let long = json_of(
            c.get(p.token_url)
                .query(&[
                    ("grant_type", "fb_exchange_token"),
                    ("client_id", &id),
                    ("client_secret", &secret),
                    ("fb_exchange_token", &short.access_token),
                ])
                .send()
                .await?,
        )
        .await?;
        let mut t = parse_token(&long, None)?;
        t.expires_at = t.expires_at.or(Some(unix_now() + 60 * 86400));
        return Ok(t);
    }
    let mut form = vec![
        ("grant_type", "authorization_code".to_string()),
        ("code", code.to_string()),
        ("redirect_uri", redirect_uri()),
        (p.id_param, id),
        ("client_secret", secret),
    ];
    if p.pkce != Pkce::None {
        form.push(("code_verifier", verifier.to_string()));
    }
    parse_token(&json_of(c.post(p.token_url).form(&form).send().await?).await?, None)
}

async fn refresh(p: &Provider, t: &TokenSet) -> Result<TokenSet> {
    let rt = t.refresh_token.clone().ok_or_else(|| anyhow!("{} sign-in expired; sign in again", p.name))?;
    let (id, secret) = app_keys(p)?;
    let form = [("grant_type", "refresh_token".to_string()), ("refresh_token", rt.clone()), (p.id_param, id), ("client_secret", secret)];
    let v = json_of(http().post(p.token_url).form(&form).send().await?)
        .await
        .with_context(|| format!("{} refused to renew the sign-in; sign in again", p.name))?;
    let mut new = parse_token(&v, Some(rt))?;
    new.refresh_expires_at = new.refresh_expires_at.or(t.refresh_expires_at);
    Ok(new)
}

/// A valid access token, refreshed if it expires within 2 minutes. Used by uploads.
pub async fn access_token(p: &Provider) -> Result<String> {
    let t = load_tokens(p)?.ok_or_else(|| anyhow!("{} isn't connected", p.name))?;
    if t.expires_at.is_some_and(|e| e - 120 < unix_now()) {
        if p.id == "meta" {
            bail!("Facebook sign-in expired (they last about 60 days); sign in again");
        }
        let new = refresh(p, &t).await?;
        store_tokens(p, &new)?;
        return Ok(new.access_token);
    }
    Ok(t.access_token)
}

// ---------- account info ----------

#[derive(Debug, Serialize)]
pub struct AccountInfo {
    pub display_name: String,
    pub account_id: Option<String>,
    pub details: Value,
}

async fn fetch_account(p: &Provider, token: &str) -> Result<AccountInfo> {
    let c = http();
    match p.id {
        "kick" => {
            let u = json_of(c.get("https://api.kick.com/public/v1/users").bearer_auth(token).send().await?).await?;
            let ch = json_of(c.get("https://api.kick.com/public/v1/channels").bearer_auth(token).send().await?).await?;
            let slug = ch["data"][0]["slug"].as_str().map(String::from);
            Ok(AccountInfo {
                display_name: u["data"][0]["name"].as_str().unwrap_or("Kick user").to_string(),
                account_id: u["data"][0]["user_id"].as_i64().map(|i| i.to_string()),
                details: json!({ "slug": slug }),
            })
        }
        "youtube" => {
            let v = json_of(
                c.get("https://www.googleapis.com/youtube/v3/channels")
                    .query(&[("part", "snippet"), ("mine", "true")])
                    .bearer_auth(token)
                    .send()
                    .await?,
            )
            .await
            .context("couldn't read your YouTube channel (is the YouTube Data API v3 enabled in your Google Cloud project?)")?;
            let ch = v["items"].get(0).ok_or_else(|| anyhow!("this Google account has no YouTube channel"))?;
            Ok(AccountInfo {
                display_name: ch["snippet"]["title"].as_str().unwrap_or("YouTube channel").to_string(),
                account_id: ch["id"].as_str().map(String::from),
                details: json!({ "custom_url": ch["snippet"]["customUrl"] }),
            })
        }
        "tiktok" => {
            let v = json_of(
                c.get("https://open.tiktokapis.com/v2/user/info/")
                    .query(&[("fields", "open_id,display_name,avatar_url")])
                    .bearer_auth(token)
                    .send()
                    .await?,
            )
            .await?;
            let u = &v["data"]["user"];
            Ok(AccountInfo {
                display_name: u["display_name"].as_str().unwrap_or("TikTok user").to_string(),
                account_id: u["open_id"].as_str().map(String::from),
                details: json!({}),
            })
        }
        "meta" => {
            let me = json_of(c.get(format!("{META_GRAPH}/me")).query(&[("fields", "id,name"), ("access_token", token)]).send().await?).await?;
            let pages = json_of(
                c.get(format!("{META_GRAPH}/me/accounts"))
                    .query(&[("fields", "id,name,instagram_business_account{id,username}"), ("access_token", token)])
                    .send()
                    .await?,
            )
            .await?;
            let pages: Vec<Value> = pages["data"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|pg| {
                    json!({
                        "id": pg["id"], "name": pg["name"],
                        "instagram_id": pg["instagram_business_account"]["id"],
                        "instagram_username": pg["instagram_business_account"]["username"],
                    })
                })
                .collect();
            Ok(AccountInfo {
                display_name: me["name"].as_str().unwrap_or("Facebook user").to_string(),
                account_id: me["id"].as_str().map(String::from),
                details: json!({ "pages": pages }),
            })
        }
        _ => bail!("unknown platform"),
    }
}

fn save_account(db: &Db, p: &Provider, info: &AccountInfo, t: &TokenSet) -> Result<()> {
    // Sign-in must be redone when the refresh token (or Meta's long-lived token) runs out.
    let expiry = if p.id == "meta" { t.expires_at } else { t.refresh_expires_at };
    let expiry = expiry.and_then(|s| chrono::DateTime::from_timestamp(s, 0)).map(|d| d.format("%Y-%m-%dT%H:%M:%SZ").to_string());
    db.conn().execute(
        "INSERT INTO accounts(platform,display_name,account_id,details,connected_at,expires_at,last_check,last_error)
         VALUES(?1,?2,?3,?4,?5,?6,?5,NULL)
         ON CONFLICT(platform) DO UPDATE SET display_name=excluded.display_name, account_id=excluded.account_id,
           details=excluded.details, connected_at=excluded.connected_at, expires_at=excluded.expires_at,
           last_check=excluded.last_check, last_error=NULL",
        rusqlite::params![p.id, info.display_name, info.account_id, info.details.to_string(), now_str(), expiry],
    )?;
    Ok(())
}

/// Full sign-in. `cancel` resolves if the user presses Cancel in the app.
pub async fn sign_in(db: &Db, p: &Provider, open: impl FnOnce(String) -> Result<()>, cancel: tokio::sync::oneshot::Receiver<()>) -> Result<String> {
    let (client_id, _) = app_keys(p)?;
    let state = random_string(32);
    let verifier = random_string(64);
    let wait = wait_for_code(&state);
    tokio::pin!(wait);
    // Bind before opening the browser so a fast redirect can't miss the listener.
    let first = tokio::time::timeout(Duration::from_millis(150), &mut wait).await;
    let code = match first {
        Ok(r) => r?,
        Err(_) => {
            open(auth_url(p, &client_id, &state, &verifier))?;
            tokio::select! {
                r = &mut wait => r?,
                _ = cancel => bail!("sign-in cancelled"),
                _ = tokio::time::sleep(SIGN_IN_TIMEOUT) => bail!("sign-in timed out after 5 minutes"),
            }
        }
    };
    let tokens = exchange_code(p, &code, &verifier).await?;
    store_tokens(p, &tokens)?;
    let info = fetch_account(p, &tokens.access_token).await?;
    save_account(db, p, &info, &tokens)?;

    // Kick: fill in the channel to monitor if it isn't set yet.
    if p.id == "kick" {
        let mut s = Settings::load(db);
        if s.kick_channel_slug.is_none() {
            if let Some(slug) = info.details["slug"].as_str() {
                s.kick_channel_slug = Some(slug.to_string());
                s.save(db)?;
            }
        }
    }
    db.log("info", "accounts", &format!("Signed in to {} as {}", p.name, info.display_name));
    Ok(info.display_name)
}

/// Renews the token if needed and re-reads the account, proving the link still works.
pub async fn check(db: &Db, p: &Provider) -> Result<String> {
    let result = async {
        let token = access_token(p).await?;
        let info = fetch_account(p, &token).await?;
        let t = load_tokens(p)?.expect("token present after access_token");
        save_account(db, p, &info, &t)?;
        Ok::<_, anyhow::Error>(info.display_name)
    }
    .await;
    if let Err(e) = &result {
        let _ = db.conn().execute(
            "UPDATE accounts SET last_check=?2, last_error=?3 WHERE platform=?1",
            rusqlite::params![p.id, now_str(), format!("{e:#}")],
        );
    }
    result
}

pub fn disconnect(db: &Db, p: &Provider) -> Result<()> {
    secrets::clear(&p.token_key())?;
    db.conn().execute("DELETE FROM accounts WHERE platform=?1", [p.id])?;
    db.log("info", "accounts", &format!("Disconnected {}", p.name));
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct Account {
    pub platform: String,
    pub display_name: String,
    pub account_id: Option<String>,
    pub details: Value,
    pub connected_at: String,
    pub expires_at: Option<String>,
    pub last_check: Option<String>,
    pub last_error: Option<String>,
}

pub fn list_accounts(db: &Db) -> Result<Vec<Account>> {
    let conn = db.conn();
    let mut st = conn.prepare("SELECT platform,display_name,account_id,details,connected_at,expires_at,last_check,last_error FROM accounts")?;
    let rows = st.query_map([], |r| {
        Ok(Account {
            platform: r.get(0)?,
            display_name: r.get(1)?,
            account_id: r.get(2)?,
            details: serde_json::from_str(&r.get::<_, String>(3)?).unwrap_or(Value::Null),
            connected_at: r.get(4)?,
            expires_at: r.get(5)?,
            last_check: r.get(6)?,
            last_error: r.get(7)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_matches_rfc7636_vector_and_tiktok_hex() {
        let v = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(code_challenge(v, Pkce::S256).unwrap(), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        let hex = code_challenge(v, Pkce::S256Hex).unwrap();
        assert_eq!(hex.len(), 64);
        assert!(hex.starts_with("13d31e96"));
        assert_eq!(code_challenge(v, Pkce::None), None);
    }

    #[test]
    fn auth_urls_use_each_platforms_conventions() {
        let tt = auth_url(provider("tiktok").unwrap(), "KEY", "st", "ver");
        assert!(tt.contains("client_key=KEY") && tt.contains("scope=user.info.basic%2Cvideo.upload"));
        let yt = auth_url(provider("youtube").unwrap(), "ID", "st", "ver");
        assert!(yt.contains("access_type=offline") && yt.contains("code_challenge_method=S256"));
        let fb = auth_url(provider("meta").unwrap(), "ID", "st", "ver");
        assert!(!fb.contains("code_challenge"));
        assert!(fb.contains("redirect_uri=http%3A%2F%2Flocalhost%3A47821%2Fcallback"));
    }

    #[test]
    fn callback_parsing() {
        assert_eq!(parse_callback("GET /callback?code=abc%2F1&state=s HTTP/1.1", "s").unwrap(), Callback::Code("abc/1".into()));
        assert_eq!(parse_callback("GET /favicon.ico HTTP/1.1", "s").unwrap(), Callback::Other);
        assert!(matches!(parse_callback("GET /callback?error=access_denied&state=s HTTP/1.1", "s").unwrap(), Callback::Denied(_)));
        assert!(parse_callback("GET /callback?code=abc&state=evil HTTP/1.1", "s").is_err());
    }

    #[test]
    fn token_parsing_keeps_old_refresh_token() {
        let t = parse_token(&json!({"access_token": "a", "expires_in": 3600}), Some("r".into())).unwrap();
        assert_eq!(t.refresh_token.as_deref(), Some("r"));
        assert!(t.expires_at.unwrap() > unix_now());
        let rotated = parse_token(&json!({"access_token": "a", "refresh_token": "r2", "refresh_expires_in": 31536000}), Some("r".into())).unwrap();
        assert_eq!(rotated.refresh_token.as_deref(), Some("r2"));
        assert!(parse_token(&json!({"error": "invalid_grant", "error_description": "bad code"}), None).is_err());
    }

    #[tokio::test]
    async fn loopback_listener_receives_code() {
        let wait = tokio::spawn(async { wait_for_code("xyz").await });
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", REDIRECT_PORT)).await.unwrap();
        s.write_all(b"GET /callback?code=C0DE&state=xyz HTTP/1.1\r\nHost: localhost\r\n\r\n").await.unwrap();
        let mut resp = String::new();
        s.read_to_string(&mut resp).await.unwrap();
        assert!(resp.starts_with("HTTP/1.1 200"));
        assert_eq!(wait.await.unwrap().unwrap(), "C0DE");
    }
}
