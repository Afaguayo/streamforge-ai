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
| Video Processing Engine   | `pipeline.rs` + `media.rs`    | metadata only |
| Transcription / Moments / Clip editing | `worker/` (Python), called as jobs | Phase 3 |
| Thumbnails / Trends / Metadata | jobs + worker            | Phase 4 |
| Publishing / Analytics    | Rust (HTTP + OAuth)           | Phase 5–6 |

### Job queue (`jobs.rs`)
- Every unit of work is a row in `jobs`, so work is resumable by design. On startup, `running` → `queued`.
- `claim_next` takes the oldest due job; the worker runs **one job at a time** to keep the PC usable while gaming.
- On failure the job is retried after 30 s, 60 s, 120 s, and so on (capped at 1 h), up to `max_attempts`, then marked `failed`.
- `enqueue` refuses a duplicate live job for the same (kind, stream).
- Add a job kind by adding a match arm in `pipeline::run`. Mark it heavy in `is_heavy` so it pauses while you're live.

### Stream lifecycle
`live` (Kick says live) → `awaiting_file` (went offline) → `queued` (recording found) → `probed` → *(Phase 3: transcribed → analysed → clipped)*.
Local imports start at `queued`.

### Conventions
- Database schema: append a new string to `MIGRATIONS` in `db.rs`. Never edit an existing entry.
- Secrets: only names listed in `secrets::KNOWN_KEYS`; never log values; HTTP errors log the status, not the body.
- Child processes go through `media::hidden()` so no console windows flash.
- UI: no button without a working action; unbuilt features render `Planned.tsx`.
- Logs the user should see go through `db.log(level, source, message)` → Dashboard → Activity.

## Tests
- `cd src-tauri && cargo test`: migrations, queue recovery/retry/dedupe, Kick parsing and session transitions, ingestion stability and matching, FFprobe parsing, settings.
- `npm test`: formatting helpers.

## Platform constraints found (verify before building each phase)
- **Kick:** an app token (client_credentials) reads `GET /public/v1/channels?slug=`. The public API has no VOD endpoint. Webhooks (`livestream.status.updated`) need a reachable endpoint, so the app polls instead.
- **TikTok:** an unaudited Content Posting API client can post only to private accounts, for at most 5 users per 24 h.
- **YouTube:** uploads from an unverified API project are private-only until audited. One upload costs about 1,600 of the default 10,000 daily quota units.
- **Facebook Reels API:** accepts 3–90 s videos. Clips over 90 s must be flagged, not cut silently, to respect the 60 s minimum.
- **Instagram:** needs a Professional account and Meta App Review for `instagram_content_publish`.
