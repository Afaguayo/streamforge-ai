import { RotateCcw, X } from "lucide-react";
import { useState } from "react";
import { api, errorText, type Job, type Stream } from "../lib/api";
import { ago } from "../lib/format";
import { JOB_LABEL, type Stage } from "../lib/stages";
import { timeLeft, useJobs } from "./Progress";
import { Badge, Button, Empty, ProgressBar } from "./ui";

export { JOB_LABEL };

export function StreamStatus({ s }: { s: Stream["status"] }) {
  const map = {
    live: <Badge tone="live">● Live</Badge>,
    awaiting_file: <Badge tone="warn">Waiting for recording</Badge>,
    downloading: <Badge tone="info">Downloading from Kick</Badge>,
    queued: <Badge tone="info">Queued</Badge>,
    probed: <Badge tone="info">Imported</Badge>,
    transcribed: <Badge tone="info">Transcribed</Badge>,
    analyzed: <Badge tone="good">Clips found</Badge>,
    error: <Badge tone="bad">Error</Badge>,
  } as const;
  return map[s] ?? <Badge>{s}</Badge>;
}

/** What something is doing, in words: state, progress with time left, and the real error when it failed. */
export function StageView({ stage, eta, compact = false }: { stage: Stage; eta?: number | null; compact?: boolean }) {
  return (
    <div className="min-w-0 space-y-1.5">
      <div className="flex flex-wrap items-center gap-2">
        <Badge tone={stage.tone}>{stage.label}</Badge>
        {stage.percent != null && (
          <span className="text-xs tabular-nums text-ink-300">
            {Math.round(stage.percent * 100)}%{eta !== undefined && ` · ${timeLeft(eta)}`}
          </span>
        )}
      </div>
      {stage.percent != null && <ProgressBar value={stage.percent} />}
      {stage.detail && !compact && stage.detail !== stage.label && <div className="text-[13px] leading-snug text-ink-300">{stage.detail}</div>}
      {stage.failed && stage.reason && (
        <div className="rounded-md bg-red-950/40 px-3 py-2 text-[13px] leading-snug text-red-200">
          <span className="font-medium">Why it failed: </span>
          <span className="break-words">{stage.reason}</span>
        </div>
      )}
    </div>
  );
}

const JOB_TONE = { queued: "info", running: "warn", done: "good", failed: "bad", cancelled: "neutral" } as const;
const JOB_STATUS: Record<Job["status"], string> = { queued: "Waiting", running: "Running", done: "Done", failed: "Failed", cancelled: "Cancelled" };

export function JobList({ jobs, onChange }: { jobs: Job[]; onChange: () => void }) {
  const [err, setErr] = useState<string | null>(null);
  const p = useJobs();
  if (jobs.length === 0) return <Empty title="No jobs here">Jobs appear when you import a recording or when your stream ends.</Empty>;

  const act = async (f: () => Promise<unknown>) => {
    try {
      await f();
      setErr(null);
      onChange();
    } catch (e) {
      setErr(errorText(e));
    }
  };

  return (
    <div className="space-y-2">
      {err && <div className="text-sm text-red-300">{err}</div>}
      {jobs.map((j) => {
        const stream = p?.streamTitle(j);
        const retryAt = j.status === "queued" && Date.parse(j.run_after) > Date.now() ? new Date(j.run_after).toLocaleTimeString() : null;
        return (
          <div key={j.id} className="flex items-start gap-4 rounded-md bg-ink-950/60 px-4 py-3">
            <div className="min-w-0 flex-1">
              <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-sm">
                <span className="font-medium">{JOB_LABEL[j.kind] ?? j.kind}</span>
                {stream && <span className="truncate text-ink-300">· {stream}</span>}
                <span className="text-xs text-ink-500">job #{j.id}</span>
              </div>
              <div className="mt-1 text-[13px] text-ink-400">
                {j.message || "No details yet"} · {j.status === "done" ? "finished" : "updated"} {ago(j.updated_at)}
                {retryAt && ` · tries again at ${retryAt}`}
              </div>
              {j.error && <div className="mt-1.5 break-words rounded bg-red-950/40 px-2.5 py-1.5 text-[13px] text-red-200">{j.error}</div>}
              {j.status === "running" && (
                <div className="mt-2 max-w-md">
                  <ProgressBar value={j.progress} />
                  <div className="mt-1 text-xs tabular-nums text-ink-300">{Math.round(j.progress * 100)}% · {timeLeft(p?.eta(j) ?? null)}</div>
                </div>
              )}
            </div>
            <div className="flex shrink-0 flex-col items-end gap-2">
              <Badge tone={JOB_TONE[j.status]}>
                {JOB_STATUS[j.status]}
                {j.attempts > 1 && ` (try ${j.attempts}/${j.max_attempts})`}
              </Badge>
              {(j.status === "failed" || j.status === "cancelled") && (
                <Button size="sm" onClick={() => act(() => api.retryJob(j.id))}><RotateCcw size={13} /> Try again</Button>
              )}
              {j.status === "queued" && (
                <Button size="sm" onClick={() => act(() => api.cancelJob(j.id))}><X size={13} /> Cancel</Button>
              )}
            </div>
          </div>
        );
      })}
    </div>
  );
}
