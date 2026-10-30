mod analysis;
mod commands;
mod db;
mod ingest;
mod jobs;
mod kick;
mod llm;
mod media;
mod oauth;
mod pipeline;
mod posts;
mod publish;
mod render;
mod research;
mod secrets;
mod settings;
mod studio;
mod tools;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, WindowEvent,
};

pub struct AppState {
    pub db: Arc<db::Db>,
    pub kick: Arc<Mutex<kick::KickState>>,
    pub kick_client: Arc<kick::KickClient>,
    pub data_dir: PathBuf,
    /// Cancels the sign-in currently waiting for the browser, if any.
    pub sign_in_cancel: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

fn show_main(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let db = Arc::new(db::Db::open(&data_dir.join("streamforge.db"))?);
            let resumed = db.recover_interrupted()?;
            db.log("info", "app", &format!("Started; {resumed} interrupted job(s) resumed"));

            let kick_state = Arc::new(Mutex::new(kick::KickState::default()));
            let kick_client = Arc::new(kick::KickClient::new());

            tauri::async_runtime::spawn(kick::monitor_loop(db.clone(), kick_client.clone(), kick_state.clone()));
            tauri::async_runtime::spawn(ingest::watch_loop(db.clone()));
            let ctx = pipeline::Ctx { db: db.clone(), data_dir: data_dir.clone() };
            tauri::async_runtime::spawn(pipeline::worker_loop(ctx, kick_state.clone()));
            tauri::async_runtime::spawn(pipeline::scheduler_loop(db.clone()));

            app.manage(AppState { db, kick: kick_state, kick_client, data_dir, sign_in_cancel: Mutex::new(None) });

            // System tray: closing the window keeps the app running in the background.
            let show = MenuItem::with_id(app, "show", "Open StreamForge AI", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            TrayIconBuilder::with_id("main")
                .icon(app.default_window_icon().cloned().expect("bundle icon"))
                .tooltip("StreamForge AI")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, ev| match ev.id.as_ref() {
                    "show" => show_main(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, ev| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                        show_main(tray.app_handle());
                    }
                })
                .build(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_overview,
            commands::list_streams,
            commands::list_jobs,
            commands::recent_activity,
            commands::retry_job,
            commands::cancel_job,
            commands::import_files,
            commands::reprobe_stream,
            commands::get_settings,
            commands::save_settings,
            commands::secret_status,
            commands::set_secret,
            commands::clear_secret,
            commands::test_kick,
            commands::detect_toolchain,
            commands::data_dir,
            commands::list_platforms,
            commands::sign_in,
            commands::cancel_sign_in,
            commands::check_account,
            commands::disconnect_account,
            studio::engine_status,
            studio::install_engine,
            studio::process_stream,
            studio::list_clips,
            studio::approve_clip,
            studio::reject_clip,
            studio::restore_clip,
            studio::edit_clip,
            studio::render_clip,
            studio::list_thumbnails,
            studio::select_thumbnail,
            studio::regenerate_thumbnails,
            studio::list_posts,
            studio::update_post,
            studio::schedule_post,
            studio::unschedule_post,
            studio::delete_post,
            studio::draft_full_video,
            studio::analytics,
            studio::refresh_stats,
            studio::latest_trend_report,
            studio::run_trend_report,
            studio::list_watchlist,
            studio::add_watch,
            studio::remove_watch,
            studio::refresh_watchlist,
            studio::clean_temp,
            studio::disk_status,
        ])
        .run(tauri::generate_context!())
        .expect("error while running StreamForge AI");
}
