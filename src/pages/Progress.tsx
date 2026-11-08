import { PauseCircle } from "lucide-react";
import { ProgressBar, timeLeft, type Progress as State } from "../components/Progress";
import { JOB_LABEL, JobList } from "../components/StreamBits";
import { Card, Empty, Notice, Page } from "../components/ui";

export default function Progress({ p }: { p: State }) {
  const { current, waiting, failed, paused } = p;
  const finished = p.jobs.filter((j) => j.status === "done" || j.status === "cancelled").slice(0, 10);
  return (
    <Page title="Progress" subtitle="What StreamForge is doing right now. Updates every second. Work runs one job at a time.">
      <div className="space-y-5">
        {paused && <Notice tone="warn"><span className="flex items-center gap-2"><PauseCircle size={15} /> {paused.message}</span></Notice>}
        <Card title="Working on">
          {current ? (
            <div>
              <div className="flex items-baseline justify-between gap-4">
                <div className="min-w-0">
                  <div className="truncate text-base font-medium">{current.message || (JOB_LABEL[current.kind] ?? current.kind)}</div>
                  <div className="truncate text-sm text-ink-300">{p.streamTitle(current) ?? JOB_LABEL[current.kind] ?? current.kind}</div>
                </div>
                <div className="text-3xl font-semibold tabular-nums">{Math.round(current.progress * 100)}%</div>
              </div>
              <div className="mt-3"><ProgressBar value={current.progress} /></div>
              <div className="mt-2 text-sm text-ink-500">{timeLeft(p.secondsLeft)}{current.attempts > 1 && ` · try ${current.attempts} of ${current.max_attempts}`}</div>
            </div>
          ) : (
            <Empty>{paused ? "Waiting to resume." : "Nothing running."}</Empty>
          )}
        </Card>
        {failed.length > 0 && <Card title={`Failed (${failed.length})`}><JobList jobs={failed} onChange={p.refresh} /></Card>}
        <Card title={`Up next (${waiting.length})`}>
          {waiting.length > 0 ? <JobList jobs={waiting} onChange={p.refresh} /> : <Empty>Nothing waiting.</Empty>}
        </Card>
        {finished.length > 0 && <Card title="Finished recently"><JobList jobs={finished} onChange={p.refresh} /></Card>}
      </div>
    </Page>
  );
}
