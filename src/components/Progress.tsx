import { Activity, AlertTriangle, CheckCircle2, PauseCircle } from "lucide-react";
import { createContext, useContext, useEffect, useRef, useState } from "react";
import type { PageId } from "../App";
import { api, type Job, type Stream } from "../lib/api";
import { remaining } from "../lib/format";
import { usePoll } from "../lib/usePoll";
import { JOB_LABEL } from "../lib/stages";
import { ProgressBar } from "./ui";

export { ProgressBar };

export type Progress = {
  jobs: Job[];
  current: Job | null;
  waiting: Job[];
  /// Failed once and scheduled to try again later.
  retrying: Job[];
  failed: Job[];
  /// Held back: a queued job's message says why (live, low disk).
  paused: Job | null;
  secondsLeft: number | null;
  streamTitle: (j: Job) => string | null;
  /// Seconds left for a job. Only the running one can be estimated.
  eta: (j: Job) => number | null;
  refresh: () => void;
};

export const ProgressContext = createContext<Progress | null>(null);
/// The shared job queue state (provided by `App`); null outside the app.
export const useJobs = () => useContext(ProgressContext);

/// Live state of the job queue. Runs once, in `App`, so the time estimate carries across pages.
export function useProgress(): Progress {
  const { data: jobs, refresh } = usePoll(api.jobs, 1000);
  const { data: streams } = usePoll(api.streams, 5000);
  const all = jobs ?? [];
  const current = all.find((j) => j.status === "running") ?? null;

  // Where the current step was first seen, to work out its speed. A step's message changes
  // when the work changes pace (the censor's second listen), so it starts a new measurement.
  const seen = useRef<{ key: string; p: number; t: number } | null>(null);
  const key = current ? `${current.id}:${current.message}` : "";
  if (current && seen.current?.key !== key) seen.current = { key, p: current.progress, t: Date.now() };

  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);

  const due = (j: Job) => Date.parse(j.run_after) <= now;
  const queued = all.filter((j) => j.status === "queued");
  const names = new Map((streams ?? []).map((s: Stream) => [s.id, s.title]));
  return {
    jobs: all,
    current,
    waiting: queued.filter(due).reverse(),
    retrying: queued.filter((j) => !due(j)),
    failed: all.filter((j) => j.status === "failed"),
    paused: current ? null : queued.find((j) => /^(Paused|Waiting:)/.test(j.message)) ?? null,
    secondsLeft: current && seen.current ? remaining(current.progress, seen.current.p, seen.current.t, now) : null,
    streamTitle: (j) => (j.stream_id != null ? names.get(j.stream_id) ?? null : null),
    eta: (j) => (current && j.id === current.id && seen.current ? remaining(current.progress, seen.current.p, seen.current.t, now) : null),
    refresh,
  };
}

export function timeLeft(s: number | null) {
  if (s == null) return "working out time left…";
  if (s < 60) return "under a minute left";
  const min = Math.ceil(s / 60);
  return min < 60 ? `about ${min} min left` : `about ${Math.floor(min / 60)} h ${min % 60} min left`;
}

/// Always-visible summary in the sidebar. Click to open the Progress page.
export function ProgressDock({ p, go }: { p: Progress; go: (page: PageId) => void }) {
  const { current, waiting, failed, paused } = p;
  return (
    <button onClick={() => go("progress")} className="block w-full bg-ink-850 px-4 py-3 text-left hover:bg-ink-800">
      {current ? (
        <>
          <div className="flex items-center justify-between gap-2 text-xs">
            <span className="flex min-w-0 items-center gap-1.5 font-medium text-ink-100">
              <Activity size={13} className="shrink-0 animate-pulse text-sky-400" />
              <span className="truncate">{current.message || (JOB_LABEL[current.kind] ?? current.kind)}</span>
            </span>
            <span className="tabular-nums text-ink-300">{Math.round(current.progress * 100)}%</span>
          </div>
          <div className="mt-1.5"><ProgressBar value={current.progress} /></div>
          <div className="mt-1.5 truncate text-xs text-ink-400">
            {timeLeft(p.secondsLeft)}{waiting.length > 0 && ` · ${waiting.length} more waiting`}
          </div>
        </>
      ) : paused ? (
        <div className="flex items-center gap-1.5 text-xs text-amber-300"><PauseCircle size={13} className="shrink-0" /><span className="truncate">{paused.message}</span></div>
      ) : (
        <div className="flex items-center gap-1.5 text-xs text-ink-500"><CheckCircle2 size={13} /> Nothing running</div>
      )}
      {failed.length > 0 && <div className="mt-1.5 flex items-center gap-1.5 text-xs text-red-300"><AlertTriangle size={12} /> {failed.length} failed</div>}
    </button>
  );
}
