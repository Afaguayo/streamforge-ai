// Turns raw statuses (stream.status, job rows, clip.render_status, post.status) into one plain
// description of what a stream, clip or post is doing right now.
import type { Clip, Job, Post, Stream } from "./api";

export type StageTone = "neutral" | "good" | "warn" | "bad" | "info" | "live";

export type Stage = {
  /** Short, plain-words state: "Transcribing speech", "Waiting for the recording", "Failed". */
  label: string;
  /** What exactly is happening (the job's own message) or what to do next. */
  detail: string | null;
  tone: StageTone;
  /** 0..1 while something is running, otherwise null. */
  percent: number | null;
  /** Index into the steps list for this kind of item. */
  step: number;
  /** Everything in the pipeline is finished. */
  finished: boolean;
  failed: boolean;
  /** The real error text when failed. */
  reason: string | null;
  /** A failed job that "Try again" can restart, when there is one. */
  retryJobId: number | null;
  running: boolean;
};

export const JOB_LABEL: Record<string, string> = {
  find_vod: "Finding the Kick VOD",
  download_vod: "Downloading from Kick",
  install_speakers: "Installing speaker detection",
  probe: "Read recording metadata",
  install_whisper: "Install transcription engine",
  transcribe: "Transcribe speech",
  analyze: "Find the best moments",
  render: "Render clip",
  thumbnails: "Make thumbnails",
  publish: "Upload post",
  upload_captions: "Upload captions",
  update_video: "Update video on YouTube",
  refresh_stats: "Refresh analytics",
  trend_report: "Trend report",
  refresh_watchlist: "Refresh watchlist",
};

/** The same jobs while they run. */
export const JOB_DOING: Record<string, string> = {
  find_vod: "Finding the Kick VOD",
  download_vod: "Downloading from Kick",
  probe: "Reading the recording",
  transcribe: "Transcribing speech",
  analyze: "Finding the best moments",
  render: "Rendering the clip",
  thumbnails: "Making thumbnails",
  publish: "Uploading",
  upload_captions: "Uploading captions",
  update_video: "Updating the video on YouTube",
};

export const STREAM_STEPS = ["Recording", "Transcribe", "Find moments", "Render clips"];
export const CLIP_STEPS = ["Picked", "Rendered", "Approved", "Posted"];
export const POST_STEPS = ["Draft", "Scheduled", "Uploading", "Live"];

const STREAM_STEP_OF: Record<string, number> = { find_vod: 0, download_vod: 0, probe: 0, transcribe: 1, analyze: 2, render: 3, thumbnails: 3 };

const base: Stage = { label: "", detail: null, tone: "neutral", percent: null, step: 0, finished: false, failed: false, reason: null, retryJobId: null, running: false };

/** The clip or post id a job was queued for (`{"clip": 4}` / `{"post": 7}`). */
export function payloadId(j: Job, key: "clip" | "post"): number | null {
  try {
    const v = JSON.parse(j.payload || "{}")[key];
    return typeof v === "number" ? v : null;
  } catch {
    return null;
  }
}

/** A failed job still counts only if the same work hasn't been redone successfully since. */
function liveFailures(jobs: Job[]): Job[] {
  return jobs.filter((f) => f.status === "failed" && !jobs.some((j) => j.kind === f.kind && j.id > f.id && j.status !== "failed" && j.status !== "cancelled"));
}

export function streamStage(s: Stream, allJobs: Job[], clips: Clip[]): Stage {
  const jobs = allJobs.filter((j) => j.stream_id === s.id);
  const failedJob = liveFailures(jobs).sort((a, b) => b.id - a.id)[0];
  const running = jobs.find((j) => j.status === "running");
  const waiting = jobs.filter((j) => j.status === "queued").sort((a, b) => a.id - b.id)[0];

  if (failedJob) {
    return { ...base, label: `Failed: ${JOB_DOING[failedJob.kind] ?? failedJob.kind}`, tone: "bad", failed: true, reason: failedJob.error ?? s.error ?? "No reason was recorded.", retryJobId: failedJob.id, step: STREAM_STEP_OF[failedJob.kind] ?? 0 };
  }
  if (s.status === "error") {
    return { ...base, label: "Failed", tone: "bad", failed: true, reason: s.error ?? "No reason was recorded.", step: 0 };
  }
  if (running) {
    return { ...base, label: JOB_DOING[running.kind] ?? running.kind, detail: running.message || null, tone: "info", percent: running.progress, step: STREAM_STEP_OF[running.kind] ?? 0, running: true };
  }
  if (waiting) {
    const retrying = !!waiting.error;
    return {
      ...base,
      label: retrying ? `Retrying ${(JOB_DOING[waiting.kind] ?? waiting.kind).toLowerCase()}` : `Waiting: ${(JOB_DOING[waiting.kind] ?? waiting.kind).toLowerCase()}`,
      detail: retrying ? `Last try failed (${waiting.error}). Try ${waiting.attempts + 1} of ${waiting.max_attempts} is scheduled.` : waiting.message || "In line behind other work.",
      tone: "warn",
      step: STREAM_STEP_OF[waiting.kind] ?? 0,
    };
  }
  switch (s.status) {
    case "live":
      return { ...base, label: "Live now", detail: "Your stream is on air. Recording starts processing after it ends.", tone: "live" };
    case "awaiting_file":
      return { ...base, label: "Waiting for the recording", detail: "The stream ended. It is picked up as soon as the file appears in your recording folder.", tone: "warn" };
    case "downloading":
      return { ...base, label: "Downloading from Kick", tone: "info" };
    case "queued":
      return { ...base, label: "In the queue", detail: "Waiting for its turn.", tone: "info" };
    case "probed":
      return { ...base, label: "Imported, not analysed yet", detail: "Press Find clips to transcribe it and pick moments.", tone: "neutral", step: 1 };
    case "transcribed":
      return { ...base, label: "Transcribed", detail: "Finding moments is next.", tone: "info", step: 2 };
    case "analyzed": {
      const mine = clips.filter((c) => c.stream_id === s.id);
      const rendering = mine.filter((c) => c.render_status === "queued").length;
      const pending = mine.filter((c) => c.status === "suggested").length;
      if (rendering > 0) return { ...base, label: `Rendering clips (${mine.length - rendering} of ${mine.length} ready)`, tone: "info", step: 3, running: true };
      const failedRenders = mine.filter((c) => c.render_status === "error").length;
      const bits = [`${mine.length} clip${mine.length === 1 ? "" : "s"} found`, pending ? `${pending} waiting for your review` : null, failedRenders ? `${failedRenders} failed to render` : null].filter(Boolean);
      return { ...base, label: pending ? "Clips ready to review" : "Done", detail: bits.join(" · "), tone: failedRenders ? "warn" : "good", step: 3, finished: true };
    }
  }
  return { ...base, label: s.status };
}

export function clipStage(c: Clip, allJobs: Job[], posts: Post[]): Stage {
  const jobs = allJobs.filter((j) => j.kind === "render" && payloadId(j, "clip") === c.id);
  const failedJob = liveFailures(jobs).sort((a, b) => b.id - a.id)[0];
  const running = jobs.find((j) => j.status === "running");
  const waiting = jobs.find((j) => j.status === "queued");
  const mine = posts.filter((p) => p.clip_id === c.id);

  if (running) return { ...base, label: "Rendering", detail: running.message || null, tone: "info", percent: running.progress, step: 0, running: true };
  if (waiting) return { ...base, label: "Waiting to render", detail: waiting.error ? `Last try failed (${waiting.error}); retrying.` : waiting.message || "In line behind other work.", tone: "warn", step: 0 };
  if (c.render_status === "error" || failedJob) {
    return { ...base, label: "Render failed", tone: "bad", failed: true, reason: c.error ?? failedJob?.error ?? "No reason was recorded.", retryJobId: failedJob?.id ?? null, step: 0 };
  }
  if (c.render_status === "queued") return { ...base, label: "Waiting to render", tone: "warn", step: 0 };
  if (c.render_status === "none") {
    return { ...base, label: "Not rendered yet", detail: "Render it to preview the 9:16 video.", tone: "neutral", step: 0 };
  }
  // Rendered: where it is in publishing.
  const uploading = mine.find((p) => p.status === "uploading");
  if (uploading) return { ...base, label: `Uploading to ${platformName(uploading.platform)}`, tone: "info", percent: uploading.progress, step: 3, running: true };
  const failedPost = mine.find((p) => p.status === "failed");
  if (failedPost) return { ...base, label: `Upload to ${platformName(failedPost.platform)} failed`, tone: "bad", failed: true, reason: failedPost.error ?? "No reason was recorded.", step: 3 };
  if (mine.some((p) => p.status === "published")) return { ...base, label: "Posted", tone: "good", step: 3, finished: true };
  if (mine.some((p) => p.status === "scheduled")) return { ...base, label: "Scheduled to post", tone: "info", step: 3 };
  if (c.status === "approved") return { ...base, label: "Approved", detail: "Drafts are waiting in Publish.", tone: "good", step: 2 };
  if (c.status === "rejected") return { ...base, label: "Rejected", tone: "neutral", step: 1 };
  return { ...base, label: "Ready for your review", detail: "Approve it to create drafts, or reject it.", tone: "good", step: 1 };
}

export function postStage(p: Post, now = Date.now()): Stage {
  const name = platformName(p.platform);
  switch (p.status) {
    case "draft":
      return { ...base, label: "Draft", detail: "Nothing is posted until you press Publish or schedule it.", tone: "neutral", step: 0 };
    case "scheduled": {
      const at = p.scheduled_at ? Date.parse(p.scheduled_at) : NaN;
      const when = isNaN(at) ? "" : `${new Date(at).toLocaleString()}${at > now ? "" : " (due now)"}`;
      return { ...base, label: "Scheduled", detail: when ? `Goes out ${when}.` : null, tone: "info", step: 1 };
    }
    case "uploading":
      return { ...base, label: `Uploading to ${name}`, detail: `${Math.round(p.progress * 100)}% sent`, tone: "info", percent: p.progress, step: 2, running: true };
    case "published":
      return { ...base, label: "Live", tone: "good", step: 3, finished: true };
    case "failed":
      return { ...base, label: "Upload failed", tone: "bad", failed: true, reason: p.error ?? "No reason was recorded.", step: 2 };
  }
  return { ...base, label: p.status };
}

export const platformName = (p: string) => ({ youtube: "YouTube", tiktok: "TikTok" } as Record<string, string>)[p] ?? p;

export const privacyLabel = (platform: string, privacy: string) =>
  privacy === "unlisted" ? (platform === "tiktok" ? "Friends only" : "Unlisted") : privacy === "public" ? "Public" : "Private";
