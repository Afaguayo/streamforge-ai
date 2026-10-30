//! Tauri commands called by the React UI. Errors are returned as strings for display.

use crate::{
    db::{Activity, Stream},
    ingest,
    jobs::{Job, JobCounts},
    kick::{KickClient, KickState},
    media, oauth, secrets,
    settings::Settings,
    AppState,
};
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use tauri::State;

type Res<T> = Result<T, String>;

fn e(err: anyhow::Error) -> String {
    format!("{err:#}")
}

#[derive(Serialize)]
pub struct Overview {
    kick: KickState,
    jobs: JobCounts,
    latest_stream: Option<Stream>,
    stream_count: usize,
    recording_dir_set: bool,
}

#[tauri::command]
pub fn get_overview(st: State<AppState>) -> Res<Overview> {
    let streams = st.db.list_streams().map_err(e)?;
    Ok(Overview {
        kick: st.kick.lock().unwrap().clone(),
        jobs: st.db.job_counts().map_err(e)?,
        stream_count: streams.len(),
        latest_stream: streams.into_iter().next(),
        recording_dir_set: Settings::load(&st.db).recording_dir.is_some(),
    })
}

#[tauri::command]
pub fn list_streams(st: State<AppState>) -> Res<Vec<Stream>> {
    st.db.list_streams().map_err(e)
}

#[tauri::command]
pub fn list_jobs(st: State<AppState>) -> Res<Vec<Job>> {
    st.db.list_jobs(200).map_err(e)
}

#[tauri::command]
pub fn recent_activity(st: State<AppState>) -> Res<Vec<Activity>> {
    st.db.recent_activity(100).map_err(e)
}

#[tauri::command]
pub fn retry_job(st: State<AppState>, id: i64) -> Res<bool> {
    st.db.job_retry(id).map_err(e)
}

#[tauri::command]
pub fn cancel_job(st: State<AppState>, id: i64) -> Res<bool> {
    st.db.job_cancel(id).map_err(e)
}

#[tauri::command]
pub fn import_files(st: State<AppState>, paths: Vec<String>) -> Res<Vec<i64>> {
    paths.iter().map(|p| ingest::ingest_file(&st.db, Path::new(p)).map_err(e)).collect()
}

#[tauri::command]
pub fn reprobe_stream(st: State<AppState>, id: i64) -> Res<i64> {
    st.db.enqueue("probe", Some(id), "{}").map_err(e)
}

#[tauri::command]
pub fn get_settings(st: State<AppState>) -> Settings {
    Settings::load(&st.db)
}

#[tauri::command]
pub fn save_settings(st: State<AppState>, settings: Settings) -> Res<Settings> {
    if let Some(dir) = &settings.recording_dir {
        if !Path::new(dir).is_dir() {
            return Err(format!("Recording folder does not exist: {dir}"));
        }
    }
    settings.save(&st.db).map_err(e)?;
    st.db.log("info", "settings", "Settings saved");
    Ok(Settings::load(&st.db))
}

/// Which credentials are stored (never their values).
#[tauri::command]
pub fn secret_status() -> HashMap<String, bool> {
    secrets::APP_KEYS.iter().map(|k| (k.to_string(), secrets::is_set(k))).collect()
}

#[tauri::command]
pub fn set_secret(st: State<AppState>, key: String, value: String) -> Res<()> {
    if !secrets::APP_KEYS.contains(&key.as_str()) {
        return Err(format!("'{key}' can't be set from the app"));
    }
    secrets::set(&key, &value).map_err(e)?;
    st.kick_client.forget_token();
    st.db.log("info", "credentials", &format!("Saved {key} to Windows Credential Manager"));
    Ok(())
}

#[tauri::command]
pub fn clear_secret(st: State<AppState>, key: String) -> Res<()> {
    if !secrets::APP_KEYS.contains(&key.as_str()) {
        return Err(format!("'{key}' can't be removed from here"));
    }
    secrets::clear(&key).map_err(e)?;
    st.kick_client.forget_token();
    st.db.log("info", "credentials", &format!("Removed {key}"));
    Ok(())
}

/// Live call to Kick with the stored credentials and slug.
#[tauri::command]
pub async fn test_kick(st: State<'_, AppState>) -> Res<String> {
    let slug = Settings::load(&st.db).kick_channel_slug.ok_or("Set your Kick channel name first")?;
    let client: &KickClient = &st.kick_client;
    client.forget_token();
    let ch = client.channel(&slug).await.map_err(e)?;
    Ok(format!(
        "Connected. kick.com/{} is {}{}",
        ch.slug,
        if ch.is_live { "LIVE" } else { "offline" },
        if ch.title.is_empty() { String::new() } else { format!(" — \"{}\"", ch.title) }
    ))
}

#[tauri::command]
pub async fn detect_toolchain() -> media::Toolchain {
    media::detect_toolchain().await
}

#[tauri::command]
pub fn data_dir(st: State<AppState>) -> String {
    st.data_dir.to_string_lossy().to_string()
}

#[derive(Serialize)]
pub struct Platform {
    id: &'static str,
    name: &'static str,
    redirect_uri: String,
    id_key: &'static str,
    secret_key: &'static str,
    keys_set: bool,
    account: Option<oauth::Account>,
}

#[tauri::command]
pub fn list_platforms(st: State<AppState>) -> Res<Vec<Platform>> {
    let mut accounts = oauth::list_accounts(&st.db).map_err(e)?;
    Ok(oauth::PROVIDERS
        .iter()
        .map(|p| Platform {
            id: p.id,
            name: p.name,
            redirect_uri: oauth::redirect_uri(),
            id_key: p.id_key,
            secret_key: p.secret_key,
            keys_set: secrets::is_set(p.id_key) && secrets::is_set(p.secret_key),
            account: accounts.iter().position(|a| a.platform == p.id).map(|i| accounts.remove(i)),
        })
        .collect())
}

/// Opens the browser for consent and waits (up to 5 min) for the redirect.
#[tauri::command]
pub async fn sign_in(app: tauri::AppHandle, st: State<'_, AppState>, platform: String) -> Res<String> {
    use tauri_plugin_opener::OpenerExt;
    let p = oauth::provider(&platform).map_err(e)?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    if let Some(old) = st.sign_in_cancel.lock().unwrap().replace(tx) {
        let _ = old.send(());
    }
    let open = |url: String| app.opener().open_url(url, None::<&str>).map_err(|err| anyhow::anyhow!("couldn't open the browser: {err}"));
    let result = oauth::sign_in(&st.db, p, open, rx).await;
    if let Err(err) = &result {
        st.db.log("warn", "accounts", &format!("{} sign-in failed: {err:#}", p.name));
    }
    result.map_err(e)
}

#[tauri::command]
pub fn cancel_sign_in(st: State<AppState>) {
    if let Some(tx) = st.sign_in_cancel.lock().unwrap().take() {
        let _ = tx.send(());
    }
}

#[tauri::command]
pub async fn check_account(st: State<'_, AppState>, platform: String) -> Res<String> {
    let p = oauth::provider(&platform).map_err(e)?;
    oauth::check(&st.db, p).await.map_err(e)
}

#[tauri::command]
pub fn disconnect_account(st: State<AppState>, platform: String) -> Res<()> {
    let p = oauth::provider(&platform).map_err(e)?;
    oauth::disconnect(&st.db, p).map_err(e)
}
