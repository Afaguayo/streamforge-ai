# Roadmap

## Phase 1 — Foundation ✅
- [x] Tauri 2 + React + TypeScript + Tailwind shell, all 12 navigation sections
- [x] SQLite with versioned migrations
- [x] Persistent job queue: crash recovery, retry with backoff, dedupe, retry/cancel in UI
- [x] System tray, close-to-tray
- [x] Settings (storage folders, polling, GPU, pause-while-live)
- [x] Account Connections with Windows Credential Manager storage
- [x] Dashboard: Kick status, latest recording, queue, activity log
- [x] NSIS installer

## Phase 2 — Kick + ingestion ✅ (Kick live polling untested against a real account)
- [x] Kick app-token auth + live/offline polling (official API)
- [x] Stream sessions recorded with title, start and end times
- [x] Recording-folder watcher with "finished writing" detection
- [x] Recording matched to the Kick session that just ended
- [x] Manual import, FFprobe metadata, GPU/NVENC detection
- [ ] Kick user OAuth (PKCE) login, needed later for writing to the channel

## Phase 3 — AI clips
- [ ] Python worker (`worker/`) with faster-whisper (CUDA, CPU fallback), word timestamps, ES/EN
- [ ] Signals: loudness/excitement, laughter, scene changes, speech rate
- [ ] LLM scoring of candidate windows (hook, entertainment, emotion, context, replay, shareability, trend)
- [ ] Clip boundaries ≥ 60 s, extended to sentence/story boundaries, never padded
- [ ] 9:16 reframing (face/webcam + gameplay layouts), loudnorm, animated word-by-word ASS subtitles
- [ ] AI Clip Studio: preview, approve/reject/edit/regenerate

## Phase 4 — Thumbnails, trends, competitors
- [ ] Frame selection + face/expression detection, background removal, layered 1280×720 compositions, 3 ranked concepts
- [ ] Vertical covers
- [ ] Trend research from official APIs (YouTube Data API first), daily report
- [ ] Competitor watchlist

## Phase 5 — Publishing
- [ ] YouTube (OAuth desktop flow, resumable upload, thumbnails)
- [ ] TikTok Content Posting API (audit status shown)
- [ ] Instagram Reels, Facebook Reels (Graph API)
- [ ] Upload queue: review-first, retry, duplicate prevention, per-platform duration checks

## Phase 6 — Analytics + automation
- [ ] Real analytics per platform, charts
- [ ] Feedback loop into clip and thumbnail scoring
- [ ] Publishing calendar, scheduling, opt-in full automation

## Phase 7 — Hardening
- [ ] Auto-update (Tauri updater + signed releases)
- [ ] Disk-space monitor and temp cleanup
- [ ] Performance pass, user guide
