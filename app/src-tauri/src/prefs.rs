//! prefs.rs - durable user preferences.
//!
//! Stores relay URL, display name, languages, and device choices in
//! `%APPDATA%\com.ollalink.translate\prefs.json` on Windows, so the user
//! doesn't have to re-enter them every launch.
//!
//! Schema is versioned - bump VERSION on breaking changes and discard old files.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const VERSION: u32 = 1;
const FILENAME: &str = "prefs.json";
pub const PRODUCTION_RELAY_URL: &str = "https://windows-live-translation-app-lzx2.onrender.com";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct UserPrefs {
    pub version: u32,
    pub display_name: String,
    pub relay_url: String,
    pub source_lang: String,
    pub target_lang: String,
    pub voice_persona: String,
    pub voice_tone: String,
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub input_volume: f32,
}

impl UserPrefs {
    fn with_defaults() -> Self {
        Self {
            version: VERSION,
            display_name: String::new(),
            relay_url: PRODUCTION_RELAY_URL.into(),
            source_lang: "auto".into(),
            target_lang: "hi".into(),
            voice_persona: "nh-m01".into(),
            voice_tone: "natural".into(),
            input_device: None,
            output_device: None,
            input_volume: 1.0,
        }
    }
}

fn prefs_path() -> Result<PathBuf> {
    let base = dirs::config_dir().context("no config directory")?;
    let dir = base.join("com.ollalink.translate");
    std::fs::create_dir_all(&dir).context("create prefs dir")?;
    Ok(dir.join(FILENAME))
}

pub fn load() -> UserPrefs {
    match prefs_path().and_then(|p| {
        let raw = std::fs::read_to_string(&p)?;
        let parsed: UserPrefs = serde_json::from_str(&raw)?;
        Ok::<_, anyhow::Error>(parsed)
    }) {
        Ok(mut p) if p.version == VERSION => {
            // Auto-upgrade legacy localhost/127.0.0.1 or old Render relays to active production cloud relay (lzx2)
            if p.relay_url.is_empty()
                || p.relay_url.contains("localhost")
                || p.relay_url.contains("127.0.0.1")
                || p.relay_url.contains("windows-live-translation-app-1")
            {
                p.relay_url = PRODUCTION_RELAY_URL.into();
                let _ = save(&p);
            }
            p
        }
        Ok(_) => UserPrefs::with_defaults(), // version mismatch - reset
        Err(_) => UserPrefs::with_defaults(),
    }
}

pub fn save(prefs: &UserPrefs) -> Result<()> {
    let mut p = prefs.clone();
    p.version = VERSION;
    let path = prefs_path()?;
    let buf = serde_json::to_string_pretty(&p)?;
    std::fs::write(path, buf).context("write prefs")?;
    Ok(())
}
