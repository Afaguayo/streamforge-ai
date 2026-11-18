//! Uploads to YouTube (Data API v3, resumable) and TikTok (Content Posting API, Direct Post).
//! Instagram and Facebook are not wired up yet (Meta developer registration pending).

use crate::oauth;
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Duration;

const CHUNK: u64 = 8 * 1024 * 1024;

pub struct Upload<'a> {
    pub file: &'a Path,
    pub title: &'a str,
    pub description: &'a str,
    pub tags: &'a [String],
    pub privacy: &'a str,
    pub publish_at: Option<&'a str>, // RFC 3339; YouTube schedules private videos to go public then
    pub thumbnail: Option<&'a Path>,
    pub duration_s: f64,
}

pub struct Published {
    pub remote_id: String,
    pub url: String,
    pub note: Option<String>,
}

fn http() -> reqwest::Client {
    reqwest::Client::builder().connect_timeout(Duration::from_secs(20)).timeout(Duration::from_secs(600)).build().expect("http")
}

fn read_chunk(f: &mut std::fs::File, offset: u64, len: u64) -> Result<Vec<u8>> {
    f.seek(SeekFrom::Start(offset))?;
    let mut buf = vec![0u8; len as usize];
    f.read_exact(&mut buf)?;
    Ok(buf)
}

async fn err_text(resp: reqwest::Response) -> String {
    let status = resp.status();
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    let msg = v["error"]["message"].as_str().or(v["error"]["errors"][0]["reason"].as_str()).unwrap_or("");
    format!("HTTP {status} {msg}")
}

const UPLOAD_RETRIES: u32 = 5;

enum Stored {
    Offset(u64),
    Done(Value),
}

/// Bytes the server holds, from a 308's `Range: bytes=0-N`; no header means none.
fn stored_offset(resp: &reqwest::Response) -> u64 {
    resp.headers()
        .get("range")
        .and_then(|r| r.to_str().ok())
        .and_then(|r| r.rsplit('-').next()?.trim().parse::<u64>().ok())
        .map(|last| last + 1)
        .unwrap_or(0)
}

/// Asks a resumable session how much it has stored.
async fn youtube_stored(session: &str, size: u64) -> Result<Stored> {
    let token = oauth::access_token(oauth::provider("youtube")?).await?;
    let resp = http()
        .put(session)
        .bearer_auth(&token)
        .header("Content-Length", 0)
        .header("Content-Range", format!("bytes */{size}"))
        .send()
        .await?;
    match resp.status().as_u16() {
        200 | 201 => Ok(Stored::Done(resp.json().await?)),
        308 => Ok(Stored::Offset(stored_offset(&resp))),
        _ => bail!("YouTube upload status check failed: {}", err_text(resp).await),
    }
}

/// YouTube rejects `<` and `>` in text fields.
fn strip_angle(s: &str) -> String {
    s.chars().filter(|c| *c != '<' && *c != '>').collect()
}

/// At most `max` bytes, cut on a char boundary.
fn truncate_bytes(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

fn yt_title(s: &str) -> String {
    truncate(strip_angle(s).trim(), 100)
}

fn yt_description(s: &str) -> String {
    truncate_bytes(&strip_angle(s), 5000)
}

fn yt_tags(tags: &[String]) -> Vec<String> {
    tags.iter().map(|t| strip_angle(t)).filter(|t| !t.trim().is_empty()).collect()
}

// ---------- YouTube ----------

pub async fn youtube(u: &Upload<'_>, mut progress: impl FnMut(f64)) -> Result<Published> {
    let p = oauth::provider("youtube")?;
    let size = std::fs::metadata(u.file)?.len();
    if size == 0 {
        bail!("the clip file is empty (0 bytes), so there is nothing to upload");
    }
    let mut status = json!({ "privacyStatus": u.privacy, "selfDeclaredMadeForKids": false });
    if let Some(at) = u.publish_at {
        status["privacyStatus"] = json!("private");
        status["publishAt"] = json!(at);
    }
    let meta = json!({
        "snippet": { "title": yt_title(u.title), "description": yt_description(u.description), "tags": yt_tags(u.tags), "categoryId": "20" },
        "status": status,
    });
    let token = oauth::access_token(p).await?;
    let init = http()
        .post("https://www.googleapis.com/upload/youtube/v3/videos?uploadType=resumable&part=snippet,status")
        .bearer_auth(&token)
        .header("X-Upload-Content-Length", size)
        .header("X-Upload-Content-Type", "video/mp4")
        .json(&meta)
        .send()
        .await?;
    if !init.status().is_success() {
        bail!("YouTube rejected the upload: {}", err_text(init).await);
    }
    let session = init.headers().get("location").and_then(|v| v.to_str().ok()).ok_or_else(|| anyhow!("YouTube gave no upload URL"))?.to_string();

    let mut f = std::fs::File::open(u.file)?;
    let mut offset = 0u64;
    let mut failures = 0u32;
    let video: Value = loop {
        let len = CHUNK.min(size - offset);
        let body = read_chunk(&mut f, offset, len)?;
        let token = oauth::access_token(p).await?; // long uploads outlive a 1 h token
        let sent = http()
            .put(&session)
            .bearer_auth(&token)
            .header("Content-Length", len)
            .header("Content-Range", format!("bytes {}-{}/{}", offset, offset + len - 1, size))
            .body(body)
            .send()
            .await;
        let resp = match sent {
            Ok(r) if !r.status().is_server_error() => r,
            other => {
                // 5xx or a dropped connection: the session survives, so ask what it stored and go on.
                failures += 1;
                let why = match other {
                    Ok(r) => err_text(r).await,
                    Err(e) => format!("{e}"),
                };
                if failures > UPLOAD_RETRIES {
                    bail!("YouTube upload interrupted and didn't recover after {UPLOAD_RETRIES} retries: {why}");
                }
                tokio::time::sleep(Duration::from_secs(1 << failures)).await;
                match youtube_stored(&session, size).await {
                    Ok(Stored::Done(v)) => break v,
                    Ok(Stored::Offset(o)) => offset = o.min(size.saturating_sub(1)),
                    Err(_) => {} // still unreachable; the next chunk attempt counts as another retry
                }
                continue;
            }
        };
        match resp.status().as_u16() {
            200 | 201 => break resp.json().await?,
            308 => {
                failures = 0;
                // Resume from what the server actually stored (no Range header: nothing yet).
                offset = stored_offset(&resp);
                progress(offset as f64 / size as f64);
            }
            _ => bail!("YouTube upload failed: {}", err_text(resp).await),
        }
    };
    let id = video["id"].as_str().ok_or_else(|| anyhow!("YouTube returned no video id"))?.to_string();
    let mut note = None;
    if video["status"]["privacyStatus"] == "private" && u.privacy != "private" && u.publish_at.is_none() {
        note = Some("YouTube kept the video private: uploads from API projects that Google hasn't audited stay private.".into());
    }
    if let Some(thumb) = u.thumbnail.filter(|t| t.is_file()) {
        if let Err(e) = youtube_thumbnail(&id, thumb).await {
            note = Some(format!("Uploaded, but {e:#}"));
        }
    }
    let url = if u.duration_s <= 180.0 { format!("https://youtube.com/shorts/{id}") } else { format!("https://youtu.be/{id}") };
    Ok(Published { remote_id: id, url, note })
}

/// Sets a video's custom thumbnail.
pub async fn youtube_thumbnail(video_id: &str, thumb: &Path) -> Result<()> {
    let token = oauth::access_token(oauth::provider("youtube")?).await?;
    let r = http()
        .post(format!("https://www.googleapis.com/upload/youtube/v3/thumbnails/set?videoId={video_id}"))
        .bearer_auth(token)
        .header("Content-Type", if thumb.extension().is_some_and(|e| e == "png") { "image/png" } else { "image/jpeg" })
        .body(std::fs::read(thumb)?)
        .send()
        .await?;
    if !r.status().is_success() {
        bail!("the thumbnail was refused ({}). Custom thumbnails need a phone-verified channel.", err_text(r).await);
    }
    Ok(())
}

const CAPTION_NAME: &str = "Transcript";

/// Uploads a subtitle track, replacing the one this app added before.
pub async fn youtube_captions(video_id: &str, language: &str, srt: &str) -> Result<()> {
    let token = oauth::access_token(oauth::provider("youtube")?).await?;
    let list: Value = http()
        .get("https://www.googleapis.com/youtube/v3/captions")
        .query(&[("part", "snippet"), ("videoId", video_id)])
        .bearer_auth(&token)
        .send()
        .await?
        .json()
        .await?;
    if let Some(msg) = list["error"]["message"].as_str() {
        bail!("YouTube refused the subtitle list: {msg}");
    }
    for c in list["items"].as_array().into_iter().flatten() {
        if c["snippet"]["name"].as_str() == Some(CAPTION_NAME) {
            if let Some(id) = c["id"].as_str() {
                let _ = http().delete("https://www.googleapis.com/youtube/v3/captions").query(&[("id", id)]).bearer_auth(&token).send().await?;
            }
        }
    }
    let meta = json!({ "snippet": { "videoId": video_id, "language": language, "name": CAPTION_NAME, "isDraft": false } });
    let boundary = "streamforge-caption-boundary";
    let body = format!(
        "--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{meta}\r\n--{boundary}\r\nContent-Type: application/octet-stream\r\n\r\n{srt}\r\n--{boundary}--\r\n"
    );
    let resp = http()
        .post("https://www.googleapis.com/upload/youtube/v3/captions?uploadType=multipart&part=snippet")
        .bearer_auth(&token)
        .header("Content-Type", format!("multipart/related; boundary={boundary}"))
        .body(body)
        .send()
        .await?;
    if !resp.status().is_success() {
        bail!("YouTube refused the subtitles: {}", err_text(resp).await);
    }
    Ok(())
}

/// Title, description, tags and privacy of a video already on YouTube.
pub struct VideoInfo {
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
    pub privacy: String,
}

pub async fn youtube_get(video_id: &str) -> Result<VideoInfo> {
    let token = oauth::access_token(oauth::provider("youtube")?).await?;
    let resp = http()
        .get("https://www.googleapis.com/youtube/v3/videos")
        .query(&[("part", "snippet,status"), ("id", video_id)])
        .bearer_auth(token)
        .send()
        .await?;
    if !resp.status().is_success() {
        bail!("YouTube couldn't read the video: {}", err_text(resp).await);
    }
    let v: Value = resp.json().await?;
    let item = v["items"].get(0).ok_or_else(|| anyhow!("YouTube has no video {video_id} on this channel"))?;
    Ok(VideoInfo {
        title: item["snippet"]["title"].as_str().unwrap_or("").into(),
        description: item["snippet"]["description"].as_str().unwrap_or("").into(),
        tags: item["snippet"]["tags"].as_array().map(|a| a.iter().filter_map(|t| t.as_str().map(String::from)).collect()).unwrap_or_default(),
        privacy: item["status"]["privacyStatus"].as_str().unwrap_or("").into(),
    })
}

/// The update request: the video's current settings with the new text and privacy. Embedding
/// and public stats are always on.
pub fn update_body(video_id: &str, item: &Value, title: &str, description: &str, tags: &[String], privacy: &str) -> Value {
    let sn = &item["snippet"];
    let st = &item["status"];
    let mut snippet = json!({
        "title": yt_title(title),
        "description": yt_description(description),
        "tags": yt_tags(tags),
        "categoryId": sn["categoryId"].as_str().unwrap_or("20"),
    });
    for k in ["defaultLanguage", "defaultAudioLanguage"] {
        if let Some(v) = sn[k].as_str() {
            snippet[k] = json!(v);
        }
    }
    let mut status = json!({
        "privacyStatus": privacy,
        "embeddable": true,
        "publicStatsViewable": true,
        "selfDeclaredMadeForKids": st["selfDeclaredMadeForKids"].as_bool().or(st["madeForKids"].as_bool()).unwrap_or(false),
    });
    if let Some(l) = st["license"].as_str() {
        status["license"] = json!(l);
    }
    json!({ "id": video_id, "snippet": snippet, "status": status })
}

/// Replaces the title, description, tags and privacy of an uploaded video, then reads it back:
/// YouTube keeps uploads from unaudited API projects private whatever is asked, and says so
/// only by not changing it.
pub async fn youtube_update(video_id: &str, title: &str, description: &str, tags: &[String], privacy: &str) -> Result<VideoInfo> {
    let token = oauth::access_token(oauth::provider("youtube")?).await?;
    // An update resets every setting it leaves out (embedding switched off this way), so the
    // current settings are read first and sent back with only these changes.
    let current: Value = http()
        .get("https://www.googleapis.com/youtube/v3/videos")
        .query(&[("part", "snippet,status"), ("id", video_id)])
        .bearer_auth(&token)
        .send()
        .await?
        .json()
        .await?;
    let item = current["items"].get(0).ok_or_else(|| anyhow!("YouTube has no video {video_id} on this channel"))?;
    let body = update_body(video_id, item, title, description, tags, privacy);
    let resp = http()
        .put("https://www.googleapis.com/youtube/v3/videos?part=snippet,status")
        .bearer_auth(token)
        .json(&body)
        .send()
        .await?;
    if !resp.status().is_success() {
        bail!("YouTube refused the change: {}", err_text(resp).await);
    }
    let now = youtube_get(video_id).await?;
    if now.privacy != privacy {
        bail!("YouTube kept the video {}. Uploads from API apps Google hasn't audited stay private.", now.privacy);
    }
    Ok(now)
}

// ---------- TikTok ----------

pub async fn tiktok_creator_info() -> Result<Value> {
    let token = oauth::access_token(oauth::provider("tiktok")?).await?;
    let v: Value = http()
        .post("https://open.tiktokapis.com/v2/post/publish/creator_info/query/")
        .bearer_auth(token)
        .header("Content-Type", "application/json; charset=UTF-8")
        .send()
        .await?
        .json()
        .await?;
    if v["error"]["code"] != "ok" {
        bail!("TikTok: {}", v["error"]["message"].as_str().unwrap_or("creator info failed"));
    }
    Ok(v["data"].clone())
}

/// TikTok chunk plan: 10 MB chunks; the last chunk absorbs the remainder (TikTok's rule).
pub fn tiktok_chunks(size: u64) -> (u64, u64) {
    const C: u64 = 10 * 1024 * 1024;
    if size <= C { (size, 1) } else { (C, size / C) }
}

pub async fn tiktok(u: &Upload<'_>, mut progress: impl FnMut(f64)) -> Result<Published> {
    let p = oauth::provider("tiktok")?;
    let info = tiktok_creator_info().await?;
    if let Some(max) = info["max_video_post_duration_sec"].as_f64() {
        if u.duration_s > max {
            bail!("this account can post videos up to {max:.0} s; the clip is {:.0} s", u.duration_s);
        }
    }
    let options: Vec<String> = info["privacy_level_options"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
    let wanted = match u.privacy { "public" => "PUBLIC_TO_EVERYONE", "unlisted" => "MUTUAL_FOLLOW_FRIENDS", _ => "SELF_ONLY" };
    let privacy = if options.iter().any(|o| o == wanted) { wanted.to_string() } else { options.first().cloned().unwrap_or("SELF_ONLY".into()) };

    let size = std::fs::metadata(u.file)?.len();
    if size == 0 {
        bail!("the clip file is empty (0 bytes), so there is nothing to upload");
    }
    let (chunk, count) = tiktok_chunks(size);
    let caption = format!("{} {}", u.title, u.tags.iter().map(|t| format!("#{}", t.trim_start_matches('#'))).collect::<Vec<_>>().join(" "));
    let token = oauth::access_token(p).await?;
    let init: Value = http()
        .post("https://open.tiktokapis.com/v2/post/publish/video/init/")
        .bearer_auth(&token)
        .header("Content-Type", "application/json; charset=UTF-8")
        .json(&json!({
            "post_info": { "title": truncate(caption.trim(), 2200), "privacy_level": privacy, "disable_comment": false, "disable_duet": false, "disable_stitch": false },
            "source_info": { "source": "FILE_UPLOAD", "video_size": size, "chunk_size": chunk, "total_chunk_count": count },
        }))
        .send()
        .await?
        .json()
        .await?;
    if init["error"]["code"] != "ok" {
        bail!("TikTok rejected the post: {}", init["error"]["message"].as_str().unwrap_or("unknown error"));
    }
    let publish_id = init["data"]["publish_id"].as_str().ok_or_else(|| anyhow!("TikTok gave no publish id"))?.to_string();
    let upload_url = init["data"]["upload_url"].as_str().ok_or_else(|| anyhow!("TikTok gave no upload URL"))?.to_string();

    let mut f = std::fs::File::open(u.file)?;
    for i in 0..count {
        let start = i * chunk;
        let len = if i == count - 1 { size - start } else { chunk };
        let body = read_chunk(&mut f, start, len)?;
        let r = http()
            .put(&upload_url)
            .header("Content-Type", "video/mp4")
            .header("Content-Length", len)
            .header("Content-Range", format!("bytes {}-{}/{}", start, start + len - 1, size))
            .body(body)
            .send()
            .await?;
        if !(r.status().is_success() || r.status().as_u16() == 206) {
            bail!("TikTok chunk upload failed: HTTP {}", r.status());
        }
        progress((i + 1) as f64 / count as f64 * 0.9);
    }
    // TikTok processes the video asynchronously.
    for _ in 0..60 {
        tokio::time::sleep(Duration::from_secs(5)).await;
        // The upload already succeeded: a hiccup here must not fail the job (a retry would re-upload).
        let Ok(token) = oauth::access_token(p).await else { continue };
        let polled = async {
            http()
                .post("https://open.tiktokapis.com/v2/post/publish/status/fetch/")
                .bearer_auth(token)
                .header("Content-Type", "application/json; charset=UTF-8")
                .json(&json!({ "publish_id": publish_id }))
                .send()
                .await?
                .json::<Value>()
                .await
        }
        .await;
        let Ok(s) = polled else { continue };
        match s["data"]["status"].as_str().unwrap_or("") {
            "PUBLISH_COMPLETE" => {
                let post_id = s["data"]["publicaly_available_post_id"][0].as_u64().map(|v| v.to_string());
                let note = (privacy == "SELF_ONLY").then(|| "Posted as private (\"Only me\"): unaudited TikTok apps can only post privately.".to_string());
                return Ok(Published {
                    url: post_id.as_ref().map(|id| format!("https://www.tiktok.com/video/{id}")).unwrap_or_else(|| "https://www.tiktok.com/".into()),
                    remote_id: post_id.unwrap_or(publish_id),
                    note,
                });
            }
            "FAILED" => bail!("TikTok couldn't process the video: {}", s["data"]["fail_reason"].as_str().unwrap_or("unknown reason")),
            _ => progress(0.95),
        }
    }
    bail!("TikTok is still processing after 5 minutes; check the TikTok app")
}

fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updates_keep_existing_settings_and_turn_embedding_on() {
        let item = json!({"snippet": {"title": "old", "categoryId": "24", "defaultAudioLanguage": "en", "channelId": "x"},
            "status": {"privacyStatus": "private", "embeddable": false, "license": "creativeCommon", "madeForKids": false}});
        let b = update_body("abc", &item, "New", "Desc", &["t".into()], "public");
        assert_eq!(b["snippet"]["categoryId"], "24");
        assert_eq!(b["snippet"]["defaultAudioLanguage"], "en");
        assert!(b["snippet"].get("channelId").is_none(), "read-only fields aren't sent");
        assert_eq!(b["status"]["embeddable"], true);
        assert_eq!(b["status"]["license"], "creativeCommon");
        assert_eq!(b["status"]["privacyStatus"], "public");
    }

    #[test]
    fn youtube_text_is_sanitised_and_byte_limited() {
        assert_eq!(yt_title("a <b> c"), "a b c");
        assert_eq!(yt_title(&"x".repeat(150)).chars().count(), 100);
        let d = yt_description(&"é".repeat(3000)); // 6000 bytes
        assert!(d.len() <= 5000 && d.len() >= 4998);
        assert!(!yt_description("<script>").contains('<'));
        assert_eq!(truncate_bytes("héllo", 2), "h", "never splits a char");
        assert_eq!(yt_tags(&["<>".into(), "ok<".into()]), vec!["ok".to_string()]);
    }

    #[test]
    fn tiktok_chunk_plan_follows_rules() {
        assert_eq!(tiktok_chunks(3_000_000), (3_000_000, 1));
        let c = 10 * 1024 * 1024;
        assert_eq!(tiktok_chunks(c * 3 + 5), (c, 3), "remainder rides on the last chunk");
    }

    /// Live: `cargo test real_youtube_privacy -- --ignored --nocapture` with STREAMFORGE_VIDEO_ID.
    #[tokio::test]
    #[ignore]
    async fn real_youtube_privacy() {
        let Ok(id) = std::env::var("STREAMFORGE_VIDEO_ID") else { return };
        let v = youtube_get(&id).await.unwrap();
        println!("before: privacy={} title={:?}", v.privacy, v.title);
        match youtube_update(&id, &v.title, &v.description, &v.tags, "public").await {
            Ok(n) => println!("after: privacy={}", n.privacy),
            Err(e) => println!("update result: {e:#}"),
        }
    }
}
