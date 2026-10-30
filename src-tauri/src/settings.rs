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
        s.kick_channel_slug = Some(" https://kick.com/MyChannel/ ".into());
        s.kick_poll_secs = 1;
        s.save(&db).unwrap();
        let s = Settings::load(&db);
        assert_eq!(s.kick_channel_slug.as_deref(), Some("mychannel"));
        assert_eq!(s.kick_poll_secs, 30);
    }
}
