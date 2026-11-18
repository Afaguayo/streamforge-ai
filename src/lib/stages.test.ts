import { describe, expect, it } from "vitest";
import type { Clip, Job, Stream } from "./api";
import { clipStage, payloadId, streamStage } from "./stages";

const stream = (o: Partial<Stream> = {}): Stream => ({ id: 1, source: "local", title: "t", kick_started: null, kick_ended: null, file_path: "a.mp4", file_size: 1, duration_s: 60, width: null, height: null, fps: null, video_codec: null, audio_codec: null, status: "probed", error: null, created_at: "2026-01-01T00:00:00Z", kick_vod_id: null, ...o });
const job = (o: Partial<Job> = {}): Job => ({ id: 1, kind: "transcribe", stream_id: 1, payload: "{}", status: "queued", progress: 0, message: "", attempts: 0, max_attempts: 3, error: null, run_after: "", created_at: "", updated_at: "", ...o });
const clip = (o: Partial<Clip> = {}): Clip => ({ id: 5, stream_id: 1, stream_title: "t", start_s: 0, end_s: 60, score: 50, scores: {} as Clip["scores"], category: null, title: null, hook: null, reason: null, transcript: null, scored_by: "heuristic", status: "suggested", render_status: "done", render_path: "c.mp4", cover_path: null, error: null, posts: [], words: [], language: "en", render_size: 10, ...o });

describe("streamStage", () => {
  it("shows percent and message for the running job", () => {
    const s = streamStage(stream(), [job({ status: "running", progress: 0.4, message: "Transcribing speech" })], []);
    expect(s.percent).toBe(0.4);
    expect(s.step).toBe(1);
    expect(s.label).toBe("Transcribing speech");
  });
  it("reports a failed job with its real reason and a retry target", () => {
    const s = streamStage(stream(), [job({ id: 9, status: "failed", error: "ffmpeg exited 1" })], []);
    expect(s.failed).toBe(true);
    expect(s.reason).toBe("ffmpeg exited 1");
    expect(s.retryJobId).toBe(9);
  });
  it("ignores a failure that was redone successfully later", () => {
    const s = streamStage(stream({ status: "transcribed" }), [job({ id: 2, status: "done" }), job({ id: 1, status: "failed", error: "x" })], []);
    expect(s.failed).toBe(false);
  });
  it("says it is waiting for the recording", () => {
    expect(streamStage(stream({ status: "awaiting_file" }), [], []).label).toBe("Waiting for the recording");
  });
  it("counts rendered clips when analysed", () => {
    const s = streamStage(stream({ status: "analyzed" }), [], [clip({ render_status: "queued" }), clip({ id: 6 })]);
    expect(s.label).toContain("1 of 2 ready");
  });
});

describe("clipStage", () => {
  it("finds its render job through the payload", () => {
    const j = job({ id: 3, kind: "render", stream_id: null, payload: '{"clip":5}', status: "running", progress: 0.5, message: "Rendering clip" });
    expect(payloadId(j, "clip")).toBe(5);
    expect(clipStage(clip({ render_status: "queued" }), [j], []).percent).toBe(0.5);
    expect(clipStage(clip({ id: 6, render_status: "queued" }), [j], []).percent).toBeNull();
  });
  it("shows the render error", () => {
    const s = clipStage(clip({ render_status: "error", error: "no space" }), [], []);
    expect(s.failed).toBe(true);
    expect(s.reason).toBe("no space");
  });
});
