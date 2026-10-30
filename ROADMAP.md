# Roadmap

## Phase 1 — Foundation ✅
- [x] Tauri 2 + React + TypeScript + Tailwind shell, all navigation sections
- [x] SQLite with versioned migrations
- [x] Persistent job queue: crash recovery, retry with backoff, dedupe, retry/cancel in UI
- [x] System tray, close-to-tray
- [x] Settings, Account Connections (Windows Credential Manager), Dashboard
- [x] NSIS installer, first-run Setup guide

## Phase 2 — Kick + ingestion ✅
- [x] Kick live/offline polling (official API) and user sign-in
- [x] Recording-folder watcher, matched to the Kick session that just ended
- [x] Manual import, FFprobe metadata, GPU/NVENC detection

## Accounts ✅
- [x] One-click sign-in: Kick ✓ tested, YouTube ✓ tested, TikTok ✓ tested (sandbox), Facebook + Instagram (built, untested: Meta registration pending)

## Phase 3 — AI clips ✅ (v0.2.0)
- [x] whisper.cpp transcription (CUDA build, CPU fallback), word timestamps, Spanish/English auto-detect; one-click install
- [x] Signals: loudness vs. the stream's own median, laughter and excitement words (ES + EN), speech density, scene changes
- [x] Candidate windows 60–120 s, edges snapped to pauses in speech, never padded; recordings < 60 s give no clips
- [x] Scores: hook, entertainment, emotion, context, replay, shareability, trend; heuristic by default, Claude (transcript-based) when an API key is set
- [x] 9:16 render: blurred-fill or facecam-on-top layout, animated word-by-word captions, on-screen hook, loudnorm, NVENC/x264 at low priority
- [x] AI Clip Studio: preview, approve/reject/restore, edit trims/title/hook, re-render
- [ ] Face/subject tracking for automatic reframing (fixed layouts for now)
- [ ] Game-event detection (kill feeds etc.)

## Phase 4 — Thumbnails, trends, competitors ✅ (v0.2.0)
- [x] Three 1280×720 thumbnail concepts per stream from its strongest moments; vertical covers per clip
- [x] Daily trend report from the YouTube Data API: views, engagement, short-form share, title patterns, words, hashtags, best hour
- [x] Competitor watchlist (YouTube): posting frequency, average views, shorts share, breakout videos
- [ ] AI image generation / background removal for thumbnails
- [ ] TikTok/Instagram trend data (no official API for other creators' content)

## Phase 5 — Publishing ✅ for YouTube + TikTok (v0.2.0)
- [x] YouTube resumable upload (8 MB chunks, resume), Shorts, full-stream upload with selected thumbnail
- [x] TikTok Direct Post (chunked, creator-info duration and privacy checks)
- [x] Upload queue: review-first drafts, edit metadata, publish now, schedule, retries, duplicate prevention, per-platform duration checks
- [x] Titles/captions/hashtags: Claude when a key is set, otherwise from the clip and trend hashtags
- [ ] Instagram Reels, Facebook Reels (needs Meta developer registration)

## Phase 6 — Analytics + automation ✅ (v0.2.0)
- [x] YouTube stats snapshots every 6 h; views over time, by platform, clip type and posting hour
- [x] Feedback loop: clip types with 3+ published clips nudge scores (+20 % / −15 %)
- [x] Publishing calendar with drag-and-drop scheduling
- [x] Opt-in full automation: best clips approved and scheduled N per day
- [ ] TikTok stats (needs audited app + video.list)

## Phase 7 — Hardening
- [x] Disk-space monitor (pauses below 2 GB), temp-file cleanup
- [ ] Auto-update (Tauri updater + signed releases)
- [ ] Code signing
