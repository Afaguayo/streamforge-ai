# Development

## Architecture
```
React UI (src/)  ──invoke──▶  Rust core (src-tauri/src/)  ──spawn──▶  FFmpeg / FFprobe
                                   │                                   Python AI worker (Phase 3+)
                                   ├── SQLite (app data dir/streamforge.db)
                                   └── Windows Credential Manager
```
The Rust core owns all state. Long-running engines are background tasks started in `lib.rs`:

| Service (spec §12)        | Where                         | Status |
|---------------------------|-------------------------------|--------|
| Kick Stream Monitor       | `kick.rs::monitor_loop`       | built  |
| VOD Ingestion             | `ingest.rs::watch_loop`       | built  |
| Video Processing Engine   | `pipeline.rs` + `media.rs`    | built  |
| Speech Transcription      | `analysis.rs` (whisper.cpp, installed by `tools.rs`) | built |
| AI Moment Detection       | `analysis.rs` + optional `llm.rs` (Claude) | built |
| Clip Editing / Thumbnails | `render.rs` (FFmpeg + ASS captions) | built |
| Metadata Generation       | `posts.rs` + `llm.rs`         | built  |
| Publishing                | `publish.rs` (YouTube, TikTok), `posts.rs` queue | built (IG/FB pending) |
| Analytics / Trends / Watchlist | `research.rs`            | built (YouTube data) |
| Account sign-in (OAuth)   | `oauth.rs`                    | built  |

### Job queue (`jobs.rs`)
- Every unit of work is a row in `jobs`, so work is resumable by design. On startup, `running` → `queued`.
- `claim_next` takes the oldest due job; the worker runs **one job at a time** to keep the PC usable while gaming.
- On failure the job is retried after 30 s, 60 s, 120 s, and so on (capped at 1 h), up to `max_attempts`, then marked `failed`.
- `enqueue` refuses a duplicate live job for the same (kind, stream).
- Add a job kind by adding a match arm in `pipeline::run`. Mark it heavy in `is_heavy` so it pauses while you're live.

### Stream lifecycle
`live` (Kick says live) → `awaiting_file` (went offline) → `downloading` (no local recording, Kick VOD found) → `queued` (file ready) → `probed` → `transcribed` → `analyzed`.

A recording attaches to the session it was written during (file modified time inside the session, plus 20 minutes), never just the newest waiting session: a stream with no recording would otherwise take the next stream's file.
Local imports start at `queued`.

### Conventions
- Database schema: append a new string to `MIGRATIONS` in `db.rs`. Never edit an existing entry.
- Secrets: only names listed in `secrets::APP_KEYS` (plus the token names); never log values; HTTP errors log the status, not the body.
- Child processes go through `media::hidden()` so no console windows flash.
- UI: no button without a working action; unbuilt features render `Planned.tsx`.
- Logs the user should see go through `db.log(level, source, message)` → Dashboard → Activity.

### Sign-in (`oauth.rs`)
- The app listens on `127.0.0.1` and `::1`, port 47821, and opens the consent page in the default browser. The browser redirects to `http://localhost:47821/callback`; the code is exchanged for tokens.
- PKCE: Kick and Google use standard S256. TikTok desktop needs **hex**-encoded SHA-256 (sent as S256). Meta uses the client secret only, then swaps the token for a ~60-day one.
- `oauth::access_token(provider)` returns a fresh token for uploads, refreshing it when it expires within 2 minutes. TikTok rotates refresh tokens; Google keeps the old one.
- Token blobs are stored as `<platform>.token` in Credential Manager. The `accounts` table stores only the display name, IDs and Pages.

### Clip pipeline
`find_vod → download_vod → probe → transcribe → analyze → render (top N) + thumbnails → [full stream upload] → [approve] → publish`. Each arrow is a queued job; `is_heavy` jobs wait while you're live and when disk space is under 2 GB.
- Transcription: `whisper-cli -ml 1 -sow -oj` gives one word per segment with millisecond offsets; stored as `[[start, end, word], …]` in `transcripts`.
- Moments: per-second features → smoothed excitement → peaks → windows (35 s lead-in, 40 s payoff) snapped to pauses, clamped to 60–120 s, non-overlapping.
- Claude (`claude-opus-5-5`, structured JSON output, `fallbacks: "default"`) re-scores candidates and writes titles/hooks only when `llm.api_key` is set; failures fall back to heuristic scores.
- Captions: ASS karaoke, one phrase (≤3 words) at a time, phrases never overlap; FFmpeg runs from the work folder so `subtitles=subs.ass` needs no path escaping.

## Tests
- `cd src-tauri && cargo test`: migrations, queue recovery/retry/dedupe, Kick parsing and session transitions, ingestion stability and matching, FFprobe parsing, settings, PKCE (RFC 7636 vector + TikTok hex), sign-in URLs, callback parsing, token refresh handling, a real loopback callback.
- `npm test`: formatting helpers.
- Real media: `STREAMFORGE_TEST_VIDEO=<≥70 s video> cargo test -- --ignored` renders both layouts (incl. NVENC), a cover and a thumbnail, and runs loudness/scene analysis.

## Platform constraints found (verify before building each phase)
- **Kick:** an app token (client_credentials) reads `GET /public/v1/channels?slug=`. The public API has no VOD endpoint, so VODs come from the website's unofficial API: `kick.com/api/v2/channels/<slug>/videos` and `kick.com/api/v1/video/<uuid>`. Send an honest User-Agent: a browser one gets a 403. Webhooks (`livestream.status.updated`) need a reachable endpoint, so the app polls instead.
- **TikTok:** an unaudited Content Posting API client can post only to private accounts, for at most 5 users per 24 h.
- **YouTube:** uploads from an unverified API project are private-only until audited. One upload costs about 1,600 of the default 10,000 daily quota units.
- **Facebook Reels API:** accepts 3–90 s videos. Clips over 90 s must be flagged, not cut silently, to respect the 60 s minimum.
- **Instagram:** needs a Professional account and Meta App Review for `instagram_content_publish`.

### Transcription and the censor
Long recordings go to whisper.cpp in 3-minute pieces, cut at the quietest moment near each boundary. On a single long input it picks one language from the opening seconds and can loop on a line for minutes: a real 116-minute stream came back 90% repeats and translated into Spanish. Pieces, `-mc 0`, voice detection at threshold 0.35 and `drop_repeats` took that to 4%. The default threshold of 0.5 lost half the speech on busy stretches.

The censor listens twice. The transcript pass found the word in 9 of the 11 spots known on that stream; a second pass without voice detection covers the rest (`censor::add_missed`). It makes transcription 3 to 4 times slower. `transcripts.engine` records which method made a transcript; older ones are redone at startup.

Censored words are stored masked, so everything built on the transcript is clean. The stars mark what gets bleeped (`Word.b`). Clip words live on `clips.words` so caption edits, hand bleeps and speakers survive a re-render. Full streams get a bleeped copy (video copied, audio re-encoded) just before upload, deleted afterwards. A stream the censor hasn't read refuses to upload.
