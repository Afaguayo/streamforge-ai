//! Credentials live in Windows Credential Manager (via `keyring`), never in SQLite or logs.

use anyhow::{anyhow, Result};
use keyring::Entry;

const SERVICE: &str = "StreamForge AI";

/// Developer-app credentials the user types in (Account Connections).
pub const APP_KEYS: &[&str] = &[
    "kick.client_id", "kick.client_secret",
    "youtube.client_id", "youtube.client_secret",
    "tiktok.client_key", "tiktok.client_secret",
    "meta.app_id", "meta.app_secret",
];
/// Sign-in tokens, written only by `oauth.rs`.
const TOKEN_KEYS: &[&str] = &["kick.token", "youtube.token", "tiktok.token", "meta.token"];

fn entry(key: &str) -> Result<Entry> {
    if !APP_KEYS.contains(&key) && !TOKEN_KEYS.contains(&key) {
        return Err(anyhow!("unknown credential '{key}'"));
    }
    Ok(Entry::new(SERVICE, key)?)
}

pub fn set(key: &str, value: &str) -> Result<()> {
    let value = value.trim();
    if value.is_empty() {
        return Err(anyhow!("value is empty"));
    }
    entry(key)?.set_password(value)?;
    Ok(())
}

pub fn get(key: &str) -> Result<Option<String>> {
    match entry(key)?.get_password() {
        Ok(v) => Ok(Some(v)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn clear(key: &str) -> Result<()> {
    match entry(key)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

pub fn is_set(key: &str) -> bool {
    matches!(get(key), Ok(Some(_)))
}
