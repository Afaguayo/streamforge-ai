// Typed wrappers around the Rust commands in src-tauri/src/commands.rs.
import { convertFileSrc, invoke } from "@tauri-apps/api/core";

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
  status: "live" | "awaiting_file" | "queued" | "probed" | "transcribed" | "analyzed" | "error";
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
  onboarded: boolean;
  whisper_model: "base" | "small";
  auto_render_top: number;
  clip_layout: "blur" | "facecam";
  facecam_rect: [number, number, number, number];
  subtitles: boolean;
  trend_keywords: string[];
  publish_platforms: string[];
  default_privacy: "private" | "unlisted" | "public";
  auto_publish: boolean;
  posts_per_day: number;
};

export type Engine = { status: { whisper_installed: boolean; model_installed: boolean; model: string }; claude: boolean; installing: boolean };

export type Scores = { hook: number; entertainment: number; emotion: number; context: number; replay: number; shareability: number; trend: number; total: number };

export type Clip = {
  id: number;
  stream_id: number;
  stream_title: string;
  start_s: number;
  end_s: number;
  score: number;
  scores: Scores;
  category: string | null;
  title: string | null;
  hook: string | null;
  reason: string | null;
  transcript: string | null;
  scored_by: "heuristic" | "claude";
  status: "suggested" | "approved" | "rejected";
  render_status: "none" | "queued" | "done" | "error";
  render_path: string | null;
  cover_path: string | null;
  error: string | null;
  posts: [string, string][];
};

export type Thumb = { id: number; stream_id: number; stream_title: string; path: string; rank: number; concept: string; selected: boolean };

export type Post = {
  id: number;
  clip_id: number | null;
  stream_id: number | null;
  platform: "youtube" | "tiktok";
  title: string;
  description: string;
  tags: string;
  privacy: "private" | "unlisted" | "public";
  scheduled_at: string | null;
  status: "draft" | "scheduled" | "uploading" | "published" | "failed";
  progress: number;
  remote_id: string | null;
  url: string | null;
  error: string | null;
  published_at: string | null;
  created_at: string;
  media_title: string | null;
  cover: string | null;
  views: number | null;
  likes: number | null;
  comments: number | null;
};

export type Analytics = {
  published: number;
  total_views: number;
  total_likes: number;
  total_comments: number;
  by_platform: [string, number, number][];
  by_category: [string, number, number][];
  by_hour: [number, number, number][];
  series: [string, number][];
  last_refresh: string | null;
};

export type TrendVideo = { id: string; title: string; channel: string; published: string; thumb: string; views: number; likes: number; comments: number; seconds: number };
export type TrendReport = {
  keywords: string;
  created_at: string;
  report: {
    videos_analyzed: number;
    median_views: number;
    avg_engagement_pct: number;
    pct_short_form: number;
    top_publish_hour_utc: number;
    title_traits: Record<string, number>;
    top_words: [string, number][];
    top_hashtags: [string, number][];
    top_videos: TrendVideo[];
  };
};

export type Watched = {
  id: number;
  handle: string;
  channel_id: string | null;
  title: string | null;
  subscribers: number | null;
  summary: {
    posts_per_week?: number;
    avg_views?: number;
    pct_shorts?: number;
    title_traits?: Record<string, number>;
    breakout?: { id: string; title: string; views: number; thumb: string } | null;
    recent?: { id: string; title: string; views: number; thumb: string; seconds: number; published: string }[];
  };
  checked_at: string | null;
  error: string | null;
};

export type Disk = { path: string; free_gb: number; total_gb: number };

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
  engine: () => invoke<Engine>("engine_status"),
  installEngine: () => invoke<number>("install_engine"),
  processStream: (id: number) => invoke<number>("process_stream", { id }),
  clips: () => invoke<Clip[]>("list_clips"),
  approveClip: (id: number) => invoke<string[]>("approve_clip", { id }),
  rejectClip: (id: number) => invoke<void>("reject_clip", { id }),
  restoreClip: (id: number) => invoke<void>("restore_clip", { id }),
  editClip: (id: number, start: number, end: number, title: string, hook: string) => invoke<void>("edit_clip", { id, start, end, title, hook }),
  renderClip: (id: number) => invoke<void>("render_clip", { id }),
  thumbnails: () => invoke<Thumb[]>("list_thumbnails"),
  selectThumbnail: (id: number) => invoke<void>("select_thumbnail", { id }),
  regenerateThumbnails: (streamId: number) => invoke<number>("regenerate_thumbnails", { streamId }),
  posts: () => invoke<Post[]>("list_posts"),
  updatePost: (id: number, title: string, description: string, tags: string[], privacy: string) => invoke<void>("update_post", { id, title, description, tags, privacy }),
  schedulePost: (id: number, at: string | null) => invoke<void>("schedule_post", { id, at }),
  unschedulePost: (id: number) => invoke<void>("unschedule_post", { id }),
  deletePost: (id: number) => invoke<void>("delete_post", { id }),
  draftFullVideo: (streamId: number) => invoke<number>("draft_full_video", { streamId }),
  analytics: () => invoke<Analytics>("analytics"),
  refreshStats: () => invoke<number>("refresh_stats"),
  trendReport: () => invoke<TrendReport | null>("latest_trend_report"),
  runTrendReport: () => invoke<number>("run_trend_report"),
  watchlist: () => invoke<Watched[]>("list_watchlist"),
  addWatch: (handle: string) => invoke<void>("add_watch", { handle }),
  removeWatch: (id: number) => invoke<void>("remove_watch", { id }),
  refreshWatchlist: () => invoke<number>("refresh_watchlist"),
  cleanTemp: () => invoke<string>("clean_temp"),
  disks: () => invoke<Disk[]>("disk_status"),
};

export const fileUrl = (path: string) => convertFileSrc(path);

export const errorText = (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e));
