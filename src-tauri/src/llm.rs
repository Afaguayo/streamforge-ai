//! Optional Claude scoring and metadata. Used only when an API key is saved
//! (`llm.api_key`); without one, clips keep their heuristic scores and auto titles.
//! Raw HTTP: Rust has no official Anthropic SDK.

use crate::secrets;
use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use std::time::Duration;

const MODEL: &str = "claude-opus-5-5";
const URL: &str = "https://api.anthropic.com/v1/messages";

pub fn available() -> bool {
    secrets::is_set("llm.api_key")
}

/// One request returning JSON that matches `schema` (structured outputs).
async fn call_json(system: &str, user: &str, schema: Value) -> Result<Value> {
    let key = secrets::get("llm.api_key")?.ok_or_else(|| anyhow!("no Claude API key saved"))?;
    let body = json!({
        "model": MODEL,
        "max_tokens": 16000,
        "fallbacks": "default",
        "output_config": { "effort": "low", "format": { "type": "json_schema", "schema": schema } },
        "system": system,
        "messages": [{ "role": "user", "content": user }],
    });
    let resp = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()?
        .post(URL)
        .header("x-api-key", key)
        .header("anthropic-version", "2026-10-21")
        .header("anthropic-beta", "server-side-fallback-2026-07-01")
        .json(&body)
        .send()
        .await
        .context("couldn't reach the Claude API")?;
    let status = resp.status();
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        return Err(anyhow!("Claude API error ({status}): {}", v["error"]["message"].as_str().unwrap_or("")));
    }
    if v["stop_reason"] == "refusal" {
        return Err(anyhow!("Claude declined this request"));
    }
    let text = v["content"]
        .as_array()
        .and_then(|c| c.iter().find(|b| b["type"] == "text"))
        .and_then(|b| b["text"].as_str())
        .ok_or_else(|| anyhow!("Claude returned no text"))?;
    serde_json::from_str(text).context("Claude's answer wasn't valid JSON")
}

pub struct LlmScore {
    pub keep: bool,
    pub scores: [f64; 7], // hook, entertainment, emotion, context, replay, shareability, trend
    pub category: String,
    pub title: String,
    pub hook: String,
    pub reason: String,
}

/// Scores candidate windows from their transcripts. `trend_notes` is today's trend summary.
pub async fn score_candidates(transcripts: &[(f64, String)], trend_notes: &str) -> Result<Vec<LlmScore>> {
    let system = "You pick short-form clips from a livestreamer's recording. Judge each candidate only from its transcript. \
A good clip makes sense to someone who never watched the stream, has a strong first 3 seconds, and pays off. \
Score each dimension 0-100. Set keep=false for clips that are boring, confusing without more context, or unsafe to post. \
Titles: natural, curiosity-driven, no clickbait lies, no hashtags, in the transcript's language, under 70 characters. \
hook: up to 6 words shown on screen for the first seconds.";
    let mut user = String::new();
    if !trend_notes.is_empty() {
        user.push_str(&format!("Current trends in this niche:\n{trend_notes}\n\n"));
    }
    for (i, (len, t)) in transcripts.iter().enumerate() {
        user.push_str(&format!("<candidate index=\"{i}\" seconds=\"{len:.0}\">\n{t}\n</candidate>\n"));
    }
    let n = |d: &str| json!({ "type": "integer", "description": d });
    let schema = json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["clips"],
        "properties": { "clips": { "type": "array", "items": {
            "type": "object", "additionalProperties": false,
            "required": ["index","keep","hook_strength","entertainment","emotion","context","replay","shareability","trend","category","title","hook","reason"],
            "properties": {
                "index": n("candidate index"), "keep": { "type": "boolean" },
                "hook_strength": n("0-100"), "entertainment": n("0-100"), "emotion": n("0-100"), "context": n("0-100"),
                "replay": n("0-100"), "shareability": n("0-100"), "trend": n("0-100, fit with current trends"),
                "category": { "type": "string", "enum": ["funny","reaction","gameplay","conversation","debate","emotional","story","moment"] },
                "title": { "type": "string" }, "hook": { "type": "string" }, "reason": { "type": "string", "description": "one sentence" }
            }}}}
    });
    let v = call_json(system, &user, schema).await?;
    let mut out: Vec<(usize, LlmScore)> = v["clips"]
        .as_array()
        .ok_or_else(|| anyhow!("no clips in Claude's answer"))?
        .iter()
        .filter_map(|c| {
            let f = |k: &str| c[k].as_f64().unwrap_or(0.0).clamp(0.0, 100.0);
            Some((c["index"].as_u64()? as usize, LlmScore {
                keep: c["keep"].as_bool().unwrap_or(true),
                scores: [f("hook_strength"), f("entertainment"), f("emotion"), f("context"), f("replay"), f("shareability"), f("trend")],
                category: c["category"].as_str().unwrap_or("moment").into(),
                title: c["title"].as_str().unwrap_or("").into(),
                hook: c["hook"].as_str().unwrap_or("").into(),
                reason: c["reason"].as_str().unwrap_or("").into(),
            }))
        })
        .collect();
    out.sort_by_key(|(i, _)| *i);
    if out.len() != transcripts.len() || out.iter().enumerate().any(|(i, (j, _))| i != *j) {
        return Err(anyhow!("Claude's answer didn't cover every candidate"));
    }
    Ok(out.into_iter().map(|(_, s)| s).collect())
}

pub struct Metadata {
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
}

pub struct VideoPackage {
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
    pub hashtags: Vec<String>,
    pub thumb_text: String,
}

/// Title, description, tags and thumbnail wording for a full stream upload.
pub async fn video_package(stream_title: &str, category: Option<&str>, highlights: &str) -> Result<VideoPackage> {
    let system = "You package a livestreamer's full stream for YouTube. Sound like a person, not an AI: no 'Dive into', no emoji spam, \
no keyword stuffing, no claims the highlights don't support. Use the highlights' language. \
title: catchy and curiosity-driven, built on a real moment from the highlights (a quote, a fail, a twist), under 70 characters, no hashtags. \
description: 2-3 short paragraphs on what happens. Don't add links or a call to follow, they're added later. \
tags: 8-15 search tags without #. hashtags: 3-5 hashtags with #, the game first, no spaces inside a hashtag. \
thumb_text: 2 to 4 punchy words for the thumbnail, different from the title. \
Text with stars is censored: never spell out or hint at a censored word.";
    let user = format!("Stream title on Kick: {stream_title}\nGame: {}\nHighlights from the transcript:\n{highlights}", category.unwrap_or("unknown"));
    let schema = json!({ "type": "object", "additionalProperties": false, "required": ["title","description","tags","hashtags","thumb_text"],
        "properties": { "title": {"type":"string"}, "description": {"type":"string"}, "tags": {"type":"array","items":{"type":"string"}},
            "hashtags": {"type":"array","items":{"type":"string"}}, "thumb_text": {"type":"string"} } });
    let v = call_json(system, &user, schema).await?;
    Ok(VideoPackage {
        hashtags: v["hashtags"].as_array().map(|a| a.iter().filter_map(|t| t.as_str().map(String::from)).collect()).unwrap_or_default(),
        title: v["title"].as_str().unwrap_or(stream_title).into(),
        description: v["description"].as_str().unwrap_or("").into(),
        tags: v["tags"].as_array().map(|a| a.iter().filter_map(|t| t.as_str().map(String::from)).collect()).unwrap_or_default(),
        thumb_text: v["thumb_text"].as_str().unwrap_or("").into(),
    })
}

/// Platform-specific title, caption and hashtags for one clip or full video.
pub async fn metadata(platform: &str, transcript: &str, current_title: &str, full_video: bool) -> Result<Metadata> {
    let kind = if full_video { "full livestream upload" } else { "short vertical clip" };
    let rules = match platform {
        "youtube" if full_video => "YouTube video: title under 90 characters; description 2-4 short paragraphs summarizing what happens, then a call to follow the Kick channel; 8-15 search tags without #.",
        "youtube" => "YouTube Shorts: title under 90 characters ending with #Shorts; description 1-2 sentences; 3-6 hashtags.",
        _ => "TikTok caption: one or two punchy lines under 150 characters followed by 3-6 relevant hashtags; tags are the hashtags without #.",
    };
    let system = "You write metadata for a streamer's own videos. Sound like a person, not an AI: no 'Dive into', no emoji spam, \
no keyword stuffing, no claims the video doesn't support. Use the transcript's language.";
    let user = format!("{rules}\nThis is a {kind}. Working title: {current_title}\nTranscript:\n{transcript}");
    let schema = json!({ "type": "object", "additionalProperties": false, "required": ["title","description","tags"],
        "properties": { "title": {"type":"string"}, "description": {"type":"string"}, "tags": {"type":"array","items":{"type":"string"}} } });
    let v = call_json(system, &user, schema).await?;
    Ok(Metadata {
        title: v["title"].as_str().unwrap_or(current_title).into(),
        description: v["description"].as_str().unwrap_or("").into(),
        tags: v["tags"].as_array().map(|a| a.iter().filter_map(|t| t.as_str().map(String::from)).collect()).unwrap_or_default(),
    })
}

// ---------- AI thumbnails (Gemini) ----------

const GEMINI_URL: &str = "https://generativelanguage.googleapis.com/v1beta/interactions";
const GEMINI_IMAGE_MODEL: &str = "gemini-nano-banana-2.1";

pub fn gemini_available() -> bool {
    secrets::is_set("gemini.api_key")
}

/// Finds the first base64 image in a Gemini answer. Walks the whole answer, so both the
/// Interactions shape (`steps[].content[]` with `type: image` and `data`) and the older
/// `generateContent` shape (`inlineData.data`) work.
pub fn find_image(v: &Value) -> Option<String> {
    match v {
        Value::Object(m) => {
            if m.get("type").and_then(|t| t.as_str()) == Some("image") {
                if let Some(d) = m.get("data").and_then(|d| d.as_str()) {
                    return Some(d.to_string());
                }
            }
            if let Some(d) = m.get("inlineData").or_else(|| m.get("inline_data")).and_then(|i| i["data"].as_str()) {
                return Some(d.to_string());
            }
            m.values().find_map(find_image)
        }
        Value::Array(a) => a.iter().find_map(find_image),
        _ => None,
    }
}

/// Turns a stream frame into a thumbnail-style image: the same scene and streamer, punched up.
/// The headline is added afterwards by FFmpeg, because image models misspell text.
pub async fn ai_thumbnail(frame_jpg: &[u8], game: &str, moment: &str) -> Result<Vec<u8>> {
    use base64::Engine;
    let key = secrets::get("gemini.api_key")?.ok_or_else(|| anyhow!("no Gemini API key saved"))?;
    let prompt = format!(
        "Turn this livestream screenshot into an eye-catching YouTube gaming thumbnail like top creators use. \
Keep the streamer from the webcam recognizable, make them large and expressive in the right half. \
Keep the game ({game}) recognizable with dramatic lighting, saturated colors, high contrast and sharp detail. \
The moment: {moment}. Leave the left third calm and darker for a headline. \
Do not add any text, letters, numbers, logos, borders or watermarks."
    );
    let b64 = base64::engine::general_purpose::STANDARD.encode(frame_jpg);
    let body = json!({
        "model": GEMINI_IMAGE_MODEL,
        "input": [
            { "type": "image", "mime_type": "image/jpeg", "data": b64 },
            { "type": "text", "text": prompt },
        ],
        "response_format": { "type": "image", "aspect_ratio": "16:9", "mime_type": "image/png" },
    });
    let resp = reqwest::Client::builder()
        .timeout(Duration::from_secs(180))
        .build()?
        .post(GEMINI_URL)
        .header("x-goog-api-key", key)
        .json(&body)
        .send()
        .await
        .context("couldn't reach the Gemini API")?;
    let status = resp.status();
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        return Err(anyhow!("Gemini API error ({status}): {}", v["error"]["message"].as_str().unwrap_or("")));
    }
    let data = find_image(&v).ok_or_else(|| anyhow!("Gemini returned no image"))?;
    Ok(base64::engine::general_purpose::STANDARD.decode(data.trim())?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_images_in_both_answer_shapes() {
        let interactions = json!({"steps": [{"type": "thought"}, {"type": "model_output", "content": [{"type": "text", "text": "ok"}, {"type": "image", "data": "QUJD"}]}]});
        assert_eq!(find_image(&interactions).as_deref(), Some("QUJD"));
        let legacy = json!({"candidates": [{"content": {"parts": [{"text": "x"}, {"inlineData": {"mimeType": "image/png", "data": "REVG"}}]}}]});
        assert_eq!(find_image(&legacy).as_deref(), Some("REVG"));
        assert_eq!(find_image(&json!({"steps": []})), None);
    }
}
