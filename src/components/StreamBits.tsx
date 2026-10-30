import { RotateCcw, X } from "lucide-react";
import { useState } from "react";
import { api, errorText, type Job, type Stream } from "../lib/api";
import { ago } from "../lib/format";
import { Badge, Empty } from "./ui";

export function StreamStatus({ s }: { s: Stream["status"] }) {
  const map = {
    live: <Badge tone="live">● Live</Badge>,
    awaiting_file: <Badge tone="warn">Waiting for recording</Badge>,
    queued: <Badge tone="info">Queued</Badge>,
    probed: <Badge tone="good">Ready for analysis</Badge>,
    error: <Badge tone="bad">Error</Badge>,
  } as const;
  return map[s] ?? <Badge>{s}</Badge>;
}

const JOB_LABEL: Record<string, string> = { probe: "Read recording metadata" };

export function JobList({ jobs, onChange }: { jobs: Job[]; onChange: () => void }) {
  const [err, setErr] = useState<string | null>(null);
  if (jobs.length === 0) return <Empty>No processing jobs yet. Import a recording or set a recording folder.</Empty>;

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
    <div className="divide-y divide-ink-800">
      {err && <div className="pb-2 text-sm text-red-300">{err}</div>}
      {jobs.map((j) => (
        <div key={j.id} className="flex items-center gap-4 py-2.5">
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2 text-sm">
              <span className="font-medium">{JOB_LABEL[j.kind] ?? j.kind}</span>
              <span className="text-ink-500">#{j.id}{j.stream_id != null && ` · stream ${j.stream_id}`}</span>
            </div>
            <div className="mt-0.5 truncate text-xs text-ink-500" title={j.error ?? j.message}>
              {j.error ? <span className="text-red-300">{j.error}</span> : j.message || "—"} · {ago(j.updated_at)}
            </div>
            {j.status === "running" && (
              <div className="mt-1.5 h-1 overflow-hidden rounded bg-ink-800">
                <div className="h-full bg-sky-400 transition-all" style={{ width: `${Math.round(j.progress * 100)}%` }} />
              </div>
            )}
          </div>
          <Badge tone={{ queued: "info", running: "warn", done: "good", failed: "bad", cancelled: "neutral" }[j.status] as never}>
            {j.status}
            {j.attempts > 1 && ` (try ${j.attempts}/${j.max_attempts})`}
          </Badge>
          {(j.status === "failed" || j.status === "cancelled") && (
            <button title="Retry" className="text-ink-300 hover:text-white" onClick={() => act(() => api.retryJob(j.id))}>
              <RotateCcw size={15} />
            </button>
          )}
          {j.status === "queued" && (
            <button title="Cancel" className="text-ink-300 hover:text-white" onClick={() => act(() => api.cancelJob(j.id))}>
              <X size={15} />
            </button>
          )}
        </div>
      ))}
    </div>
  );
}
