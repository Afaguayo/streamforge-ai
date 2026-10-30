# StreamForge AI

Windows desktop app that turns Kick livestreams into YouTube videos, 60–120 s vertical clips, thumbnails and social posts.

**Status: Phase 1–2 plus account sign-in (foundation, Kick monitoring, recording ingestion, one-click sign-in for every platform).** AI clipping, thumbnails, trend research and publishing are not built yet; see [ROADMAP.md](ROADMAP.md). Pages for unbuilt features say so; nothing in the app shows mock data.

## What works today
- Desktop app with system tray: closing the window keeps it running; quit from the tray.
- SQLite database with a persistent job queue: jobs survive crashes and restarts, retry with backoff, and can be retried or cancelled from the UI.
- Kick live detection through Kick's official public API, recorded as stream sessions.
- Recording ingestion: watches your OBS recording folder, waits until a file has finished writing, matches it to the Kick session that just ended, and reads its metadata with FFprobe. Manual import also works.
- One-click sign-in for **Kick, YouTube, TikTok and Facebook + Instagram**. The app opens your browser, you approve, and it stays connected: tokens live in Windows Credential Manager and renew automatically. A **Check** button confirms each link still works.
- GPU detection (NVIDIA + NVENC) and a "pause heavy work while live" setting.

## Install
Download `StreamForge.AI_<version>_x64-setup.exe` from the [latest release](../../releases/latest) and run it. It installs per user, with no admin rights needed.
The installer isn't code-signed yet, so Windows SmartScreen may warn: choose **More info → Run anyway**.

**Requires FFmpeg on PATH** (`winget install Gyan.FFmpeg`). Settings → Performance shows whether it was found.

## Setup
1. **Settings → Recording folder:** choose the folder OBS records to (OBS → Settings → Output → Recording Path). Use MKV or MP4.
2. **Account Connections:** for each platform, open **Developer app (one-time setup)** and follow the three steps shown, then press **Sign in with …**.
   Every platform requires uploads to go through a developer app that you own, so you create one per platform once (about 5 minutes each).
   The redirect URL to register is `http://localhost:47821/callback`. Signing in to Kick also fills in the channel to monitor.
3. Stream as usual. When you go offline, the session shows "Waiting for recording". When OBS finishes writing the file, it's attached and processed.

## Why there is no direct Kick VOD download
Kick's public API (docs.kick.com) has no VOD or past-broadcast endpoint. Unofficial endpoints exist but are undocumented and break without notice, so StreamForge doesn't use them. Your local OBS recording is also higher quality than Kick's re-encoded VOD.

## Build from source
Needs Node 20+, the Rust stable toolchain (MSVC), Visual Studio C++ Build Tools, and WebView2 (built into Windows 11).
```
npm install
npm run app:dev      # run in development
npm run app:build    # release build + NSIS installer
npm test             # frontend tests
cd src-tauri && cargo test   # backend tests
```

See [DEVELOPMENT.md](DEVELOPMENT.md) for architecture and conventions.
