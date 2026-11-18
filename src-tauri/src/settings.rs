//! User settings, stored as one JSON document in the `settings` table.

use crate::db::Db;
use anyhow::Result;
use serde::{Deserialize, Serialize};

const KEY: &str = "app_settings";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Folder OBS (or another recorder) writes stream recordings to.
    pub recording_dir: Option<String>,
    /// Where generated clips, thumbnails and temp files go.
    pub output_dir: Option<String>,
    /// Kick channel slug, e.g. `mychannel` from kick.com/mychannel.
    pub kick_channel_slug: Option<String>,
    pub kick_poll_secs: u64,
    /// Use NVENC when available; falls back to CPU (libx264) otherwise.
    pub use_gpu: bool,
    /// Pause heavy processing while a Kick stream is live.
    pub pause_while_live: bool,
    /// Every post waits for manual approval. Full automation is opt-in (Phase 6).
    pub review_before_publish: bool,
    /// The first-run setup wizard was finished or skipped.
    pub onboarded: bool,
    /// Whisper model: "base" (fast, 142 MB) or "small" (better Spanish/English, 466 MB).
    pub whisper_model: String,
    /// How many of the best suggested clips are rendered automatically after analysis.
    pub auto_render_top: u32,
    /// "blur": gameplay centred over a blurred fill. "facecam": webcam on top, gameplay below.
    pub clip_layout: String,
    /// Webcam area in the source as fractions of width/height: [x, y, w, h].
    pub facecam_rect: [f64; 4],
    /// Burn animated word-by-word subtitles into clips.
    pub subtitles: bool,
    /// Keywords for the daily trend report (e.g. "valorant clips", "just chatting español").
    pub trend_keywords: Vec<String>,
    /// Platforms a clip gets a draft post for when it's approved.
    pub publish_platforms: Vec<String>,
    /// YouTube privacy for new posts: private | unlisted | public.
    pub default_privacy: String,
    /// Full automation: approved clips are scheduled without review. Off by default.
    pub auto_publish: bool,
    /// Spacing for automatic scheduling.
    pub posts_per_day: u32,
    /// Mask slurs in transcripts and captions and bleep them in the audio.
    pub censor_enabled: bool,
    /// "beep" or "mute".
    pub censor_style: String,
    /// Extra whole words to censor.
    pub censor_words: Vec<String>,
    /// Pixelate on-screen nudity: "off", "boxes" (just the detected areas) or "frame" (the whole picture).
    pub nudity_censor: String,
    /// Detector confidence needed, 0.3-0.95. Lower catches more but flags more game footage.
    pub nudity_threshold: f64,
    /// When a Kick stream ends, upload the full stream to YouTube.
    pub auto_upload_vod: bool,
    /// YouTube privacy for those uploads: private | unlisted | public.
    pub vod_privacy: String,
    /// Links added to the end of every YouTube description.
    pub socials: Socials,
    /// Hashtags added to every full stream description, after the game's own.
    pub hashtags: Vec<String>,
    /// Cut long stretches where nothing happens from full stream uploads.
    pub cut_dead_air: bool,
}

/// Empty entries are left out of descriptions.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Socials {
    pub kick: String,
    pub tiktok: String,
    pub instagram: String,
    pub x: String,
    pub discord: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            recording_dir: None,
            output_dir: None,
            kick_channel_slug: None,
            kick_poll_secs: 60,
            use_gpu: true,
            pause_while_live: true,
            review_before_publish: true,
            onboarded: false,
            whisper_model: "small".into(),
            // Every suggested clip (analysis keeps up to 12): with fewer, the rest showed up in
            // Clip Studio as empty "not rendered" cards.
            auto_render_top: 12,
            clip_layout: "blur".into(),
            facecam_rect: [0.70, 0.04, 0.28, 0.30],
            subtitles: true,
            trend_keywords: Vec::new(),
            publish_platforms: vec!["youtube".into(), "tiktok".into()],
            default_privacy: "private".into(),
            auto_publish: false,
            posts_per_day: 2,
            censor_enabled: true,
            censor_style: "beep".into(),
            censor_words: Vec::new(),
            nudity_censor: "off".into(),
            nudity_threshold: 0.8,
            auto_upload_vod: true,
            vod_privacy: "private".into(),
            socials: Socials::default(),
            hashtags: vec!["#kick".into(), "#livestream".into(), "#gaming".into()],
            cut_dead_air: true,
        }
    }
}

impl Settings {
    pub fn load(db: &Db) -> Settings {
        db.get_setting(KEY)
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, db: &Db) -> Result<()> {
        let mut s = self.clone();
        s.kick_poll_secs = s.kick_poll_secs.clamp(30, 900);
        s.auto_render_top = s.auto_render_top.min(20);
        s.posts_per_day = s.posts_per_day.clamp(1, 24);
        if !["base", "small"].contains(&s.whisper_model.as_str()) { s.whisper_model = "small".into(); }
        if !["blur", "facecam"].contains(&s.clip_layout.as_str()) { s.clip_layout = "blur".into(); }
        if !["private", "unlisted", "public"].contains(&s.default_privacy.as_str()) { s.default_privacy = "private".into(); }
        if !["private", "unlisted", "public"].contains(&s.vod_privacy.as_str()) { s.vod_privacy = "private".into(); }
        if !["beep", "mute"].contains(&s.censor_style.as_str()) { s.censor_style = "beep".into(); }
        if !["off", "boxes", "frame"].contains(&s.nudity_censor.as_str()) { s.nudity_censor = "off".into(); }
        s.nudity_threshold = if s.nudity_threshold.is_finite() { s.nudity_threshold.clamp(0.3, 0.95) } else { 0.8 };
        s.censor_words = s.censor_words.iter().map(|k| k.trim().to_string()).filter(|k| !k.is_empty()).collect();
        s.hashtags = s.hashtags.iter().map(|h| crate::posts::hashtag(h)).filter(|h| h.len() > 1).collect();
        s.trend_keywords = s.trend_keywords.iter().map(|k| k.trim().to_string()).filter(|k| !k.is_empty()).take(5).collect();
        s.kick_channel_slug = s.kick_channel_slug.map(|v| v.trim().trim_start_matches("https://kick.com/").trim_matches('/').to_lowercase()).filter(|v| !v.is_empty());
        db.set_setting(KEY, &serde_json::to_string(&s)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_then_roundtrip_with_normalisation() {
        let db = Db::memory().unwrap();
        let mut s = Settings::load(&db);
        assert!(s.review_before_publish);
        assert_eq!(s.auto_render_top, 12, "all suggested clips render by default");
        s.kick_channel_slug = Some(" https://kick.com/MyChannel/ ".into());
        s.kick_poll_secs = 1;
        s.save(&db).unwrap();
        let s = Settings::load(&db);
        assert_eq!(s.kick_channel_slug.as_deref(), Some("mychannel"));
        assert_eq!(s.kick_poll_secs, 30);
    }
}
