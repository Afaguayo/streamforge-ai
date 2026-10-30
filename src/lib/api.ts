// Typed wrappers around the Rust commands in src-tauri/src/commands.rs.
import { invoke } from "@tauri-apps/api/core";

export type KickState = {
  configured: boolean;
  live: boolean;
  slug: string | null;
  title: string | null;
  started_at: string | null;
  viewers: number | null;
  category: string | null;
  last_checked: string | null;
  last_error: string | null;
};

export type Stream = {
  id: number;
  source: "local" | "kick";
  title: string;
  kick_started: string | null;
  kick_ended: string | null;
  file_path: string | null;
  file_size: number | null;
  duration_s: number | null;
  width: number | null;
  height: number | null;
  fps: number | null;
  video_codec: string | null;
  audio_codec: string | null;
  status: "live" | "awaiting_file" | "queued" | "probed" | "error";
  error: string | null;
  created_at: string;
};

export type Job = {
  id: number;
  kind: string;
  stream_id: number | null;
  status: "queued" | "running" | "done" | "failed" | "cancelled";
  progress: number;
  message: string;
  attempts: number;
  max_attempts: number;
  error: string | null;
  run_after: string;
  created_at: string;
  updated_at: string;
};

export type JobCounts = { queued: number; running: number; failed: number; done: number };

export type Overview = {
  kick: KickState;
  jobs: JobCounts;
  latest_stream: Stream | null;
  stream_count: number;
  recording_dir_set: boolean;
};

export type Activity = { id: number; ts: string; level: "info" | "warn" | "error"; source: string; message: string };

export type Settings = {
  recording_dir: string | null;
  output_dir: string | null;
  kick_channel_slug: string | null;
  kick_poll_secs: number;
  use_gpu: boolean;
  pause_while_live: boolean;
  review_before_publish: boolean;
};

export type Toolchain = { ffmpeg: string | null; ffprobe: string | null; nvenc: boolean; gpu: string | null };

export type Account = {
  platform: string;
  display_name: string;
  account_id: string | null;
  details: Record<string, unknown>;
  connected_at: string;
  expires_at: string | null;
  last_check: string | null;
  last_error: string | null;
};

export type Platform = {
  id: "kick" | "youtube" | "tiktok" | "meta";
  name: string;
  redirect_uri: string;
  id_key: string;
  secret_key: string;
  keys_set: boolean;
  account: Account | null;
};

export const api = {
  overview: () => invoke<Overview>("get_overview"),
  streams: () => invoke<Stream[]>("list_streams"),
  jobs: () => invoke<Job[]>("list_jobs"),
  activity: () => invoke<Activity[]>("recent_activity"),
  retryJob: (id: number) => invoke<boolean>("retry_job", { id }),
  cancelJob: (id: number) => invoke<boolean>("cancel_job", { id }),
  importFiles: (paths: string[]) => invoke<number[]>("import_files", { paths }),
  reprobe: (id: number) => invoke<number>("reprobe_stream", { id }),
  settings: () => invoke<Settings>("get_settings"),
  saveSettings: (settings: Settings) => invoke<Settings>("save_settings", { settings }),
  secretStatus: () => invoke<Record<string, boolean>>("secret_status"),
  setSecret: (key: string, value: string) => invoke<void>("set_secret", { key, value }),
  clearSecret: (key: string) => invoke<void>("clear_secret", { key }),
  testKick: () => invoke<string>("test_kick"),
  toolchain: () => invoke<Toolchain>("detect_toolchain"),
  dataDir: () => invoke<string>("data_dir"),
  platforms: () => invoke<Platform[]>("list_platforms"),
  signIn: (platform: string) => invoke<string>("sign_in", { platform }),
  cancelSignIn: () => invoke<void>("cancel_sign_in"),
  checkAccount: (platform: string) => invoke<string>("check_account", { platform }),
  disconnect: (platform: string) => invoke<void>("disconnect_account", { platform }),
};

export const errorText = (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e));
