//! User settings, stored as one JSON document in the `settings` table.

use crate::db::Db;
use anyhow::Result;
use serde::{Deserialize, Serialize};

const KEY: &str = "app_settings";

/// Where the webcam sits in the owner's OBS scene: left edge, a third of the way down,
/// 554x311 px of 1920x1080 (measured on the recordings).
pub const FACECAM_DEFAULT: [f64; 4] = [0.0, 0.34, 0.29, 0.28];
/// The first default pointed at the top-right corner, which is gameplay in those recordings.
/// Settings saved before it changed carry it, so it's treated as never set.
const FACECAM_OLD_DEFAULT: [f64; 4] = [0.70, 0.04, 0.28, 0.30];

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
    /// Games whose full streams are never cut, matched against the Kick category (part of the
    /// name, any case). Story games have long quiet stretches that are cutscenes, not dead air.
    pub uncut_games: Vec<String>,
}

impl Settings {
    /// Whether dead air should be cut from a stream of this Kick category.
    pub fn cuts_dead_air(&self, category: Option<&str>) -> bool {
        let cat = category.unwrap_or("").to_lowercase();
        self.cut_dead_air && !self.uncut_games.iter().any(|g| !g.trim().is_empty() && cat.contains(&g.trim().to_lowercase()))
    }
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
            facecam_rect: FACECAM_DEFAULT,
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
            uncut_games: vec!["Grand Theft Auto".into()],
        }
    }
}

/// Platform ids a clip can be drafted for (the ones `posts` knows about).
const PLATFORMS: [&str; 4] = ["youtube", "tiktok", "instagram", "facebook"];

/// Each value into 0..=1; a NaN anywhere means the whole box was never set properly.
fn clamp_rect(r: [f64; 4]) -> [f64; 4] {
    if r.iter().any(|v| v.is_nan()) {
        return FACECAM_DEFAULT;
    }
    r.map(|v| v.clamp(0.0, 1.0))
}

/// `mychannel` from `mychannel`, `@MyChannel`, or any kick.com URL form (scheme, www., path, query, fragment).
fn kick_slug(v: &str) -> String {
    let v = v.trim();
    let v = v.split_once("://").map_or(v, |(_, rest)| rest);
    let v = v.split(['?', '#']).next().unwrap_or("");
    let v = v.strip_prefix("www.").unwrap_or(v);
    let v = v.strip_prefix("kick.com").unwrap_or(v);
    v.trim_matches('/').split('/').next().unwrap_or("").trim_start_matches('@').to_lowercase()
}

impl Settings {
    pub fn load(db: &Db) -> Settings {
        db.get_setting(KEY)
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str::<Settings>(&s).ok())
            .map(|mut s| {
                if s.facecam_rect == FACECAM_OLD_DEFAULT {
                    s.facecam_rect = FACECAM_DEFAULT;
                }
                s
            })
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
        s.uncut_games = s.uncut_games.iter().map(|g| g.trim().to_string()).filter(|g| !g.is_empty()).collect();
        s.censor_words = s.censor_words.iter().map(|k| k.trim().to_string()).filter(|k| !k.is_empty()).take(500).collect();
        s.facecam_rect = clamp_rect(s.facecam_rect);
        s.publish_platforms = s.publish_platforms.iter().map(|p| p.trim().to_lowercase()).filter(|p| PLATFORMS.contains(&p.as_str())).fold(Vec::new(), |mut v, p| {
            if !v.contains(&p) { v.push(p); }
            v
        });
        s.hashtags = s.hashtags.iter().map(|h| crate::posts::hashtag(h)).filter(|h| h.len() > 1).collect();
        s.trend_keywords = s.trend_keywords.iter().map(|k| k.trim().to_string()).filter(|k| !k.is_empty()).take(5).collect();
        s.kick_channel_slug = s.kick_channel_slug.map(|v| kick_slug(&v)).filter(|v| !v.is_empty());
        db.set_setting(KEY, &serde_json::to_string(&s)?)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn gta_streams_are_never_cut() {
        let s = super::Settings::default();
        assert!(!s.cuts_dead_air(Some("Grand Theft Auto VI (GTA)")));
        assert!(s.cuts_dead_air(Some("Deadlock")));
        assert!(s.cuts_dead_air(None));
        let off = super::Settings { cut_dead_air: false, ..Default::default() };
        assert!(!off.cuts_dead_air(Some("Deadlock")));
    }

    use super::*;

    #[test]
    fn kick_slug_accepts_any_url_form() {
        for i in ["MyChannel", " https://kick.com/MyChannel/ ", "http://www.kick.com/mychannel?x=1", "kick.com/mychannel#a", "https://kick.com/mychannel/videos", "@mychannel"] {
            assert_eq!(kick_slug(i), "mychannel", "{i}");
        }
    }

    #[test]
    fn facecam_rect_is_clamped_and_nan_rejected() {
        assert_eq!(clamp_rect([-1.0, 0.5, 2.0, 1.0]), [0.0, 0.5, 1.0, 1.0]);
        assert_eq!(clamp_rect([0.1, f64::NAN, 0.2, 0.3]), FACECAM_DEFAULT);
    }

    #[test]
    fn save_filters_platforms_and_caps_censor_words() {
        let db = Db::memory().unwrap();
        let s = Settings {
            publish_platforms: vec!["YouTube".into(), "myspace".into(), "youtube".into()],
            censor_words: (0..600).map(|i| format!("w{i}")).collect(),
            ..Default::default()
        };
        s.save(&db).unwrap();
        let s = Settings::load(&db);
        assert_eq!(s.publish_platforms, vec!["youtube".to_string()]);
        assert_eq!(s.censor_words.len(), 500);
    }

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

    #[test]
    fn old_webcam_default_moves_to_the_measured_one() {
        let db = Db::memory().unwrap();
        db.set_setting(KEY, r#"{"clip_layout":"facecam","facecam_rect":[0.7,0.04,0.28,0.3]}"#).unwrap();
        let s = Settings::load(&db);
        assert_eq!(s.facecam_rect, FACECAM_DEFAULT);
        assert_eq!(s.clip_layout, "facecam");
        db.set_setting(KEY, r#"{"facecam_rect":[0.6,0.5,0.3,0.3]}"#).unwrap();
        assert_eq!(Settings::load(&db).facecam_rect, [0.6, 0.5, 0.3, 0.3], "a box you set is kept");
    }
}
