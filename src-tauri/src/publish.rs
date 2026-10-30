//! Uploads to YouTube (Data API v3, resumable) and TikTok (Content Posting API, Direct Post).
//! Instagram and Facebook are not wired up yet (Meta developer registration pending).

use crate::oauth;
use anyhow::{anyhow, bail, Context, Result};
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

// ---------- YouTube ----------

pub async fn youtube(u: &Upload<'_>, mut progress: impl FnMut(f64)) -> Result<Published> {
    let p = oauth::provider("youtube")?;
    let size = std::fs::metadata(u.file)?.len();
    let mut status = json!({ "privacyStatus": u.privacy, "selfDeclaredMadeForKids": false });
    if let Some(at) = u.publish_at {
        status["privacyStatus"] = json!("private");
        status["publishAt"] = json!(at);
    }
    let meta = json!({
        "snippet": { "title": truncate(u.title, 100), "description": truncate(u.description, 4900), "tags": u.tags, "categoryId": "20" },
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
    let video: Value = loop {
        let len = CHUNK.min(size - offset);
        let body = read_chunk(&mut f, offset, len)?;
        let token = oauth::access_token(p).await?; // long uploads outlive a 1 h token
        let resp = http()
            .put(&session)
            .bearer_auth(&token)
            .header("Content-Length", len)
            .header("Content-Range", format!("bytes {}-{}/{}", offset, offset + len - 1, size))
            .body(body)
            .send()
            .await
            .context("upload interrupted")?;
        match resp.status().as_u16() {
            200 | 201 => break resp.json().await?,
            308 => {
                // Resume from what the server actually stored.
                offset = resp
                    .headers()
                    .get("range")
                    .and_then(|r| r.to_str().ok())
                    .and_then(|r| r.rsplit('-').next()?.parse::<u64>().ok())
                    .map(|last| last + 1)
                    .unwrap_or(offset + len);
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
        let token = oauth::access_token(p).await?;
        let r = http()
            .post(format!("https://www.googleapis.com/upload/youtube/v3/thumbnails/set?videoId={id}"))
            .bearer_auth(token)
            .header("Content-Type", if thumb.extension().is_some_and(|e| e == "png") { "image/png" } else { "image/jpeg" })
            .body(std::fs::read(thumb)?)
            .send()
            .await?;
        if !r.status().is_success() {
            note = Some(format!("Uploaded, but the thumbnail was refused ({}). Custom thumbnails need a phone-verified channel.", err_text(r).await));
        }
    }
    let url = if u.duration_s <= 180.0 { format!("https://youtube.com/shorts/{id}") } else { format!("https://youtu.be/{id}") };
    Ok(Published { remote_id: id, url, note })
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
        let token = oauth::access_token(p).await?;
        let s: Value = http()
            .post("https://open.tiktokapis.com/v2/post/publish/status/fetch/")
            .bearer_auth(token)
            .header("Content-Type", "application/json; charset=UTF-8")
            .json(&json!({ "publish_id": publish_id }))
            .send()
            .await?
            .json()
            .await?;
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
    fn tiktok_chunk_plan_follows_rules() {
        assert_eq!(tiktok_chunks(3_000_000), (3_000_000, 1));
        let c = 10 * 1024 * 1024;
        assert_eq!(tiktok_chunks(c * 3 + 5), (c, 3), "remainder rides on the last chunk");
    }
}
