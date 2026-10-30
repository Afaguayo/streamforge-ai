//! Credentials live in Windows Credential Manager (via `keyring`), never in SQLite or logs.

use anyhow::{anyhow, Result};
use keyring::Entry;

const SERVICE: &str = "StreamForge AI";

/// Only these names may be stored, so the UI cannot write arbitrary entries.
pub const KNOWN_KEYS: &[&str] = &["kick.client_id", "kick.client_secret"];

fn entry(key: &str) -> Result<Entry> {
    if !KNOWN_KEYS.contains(&key) {
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
