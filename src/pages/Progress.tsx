import { PauseCircle, Trash2 } from "lucide-react";
import { api } from "../lib/api";
import { timeLeft, type Progress as State } from "../components/Progress";
import { JOB_LABEL, JobList } from "../components/StreamBits";
import { Button, Card, Empty, Notice, Page, ProgressBar, useConfirm } from "../components/ui";

export default function Progress({ p }: { p: State }) {
  const { current, waiting, failed, paused, retrying } = p;
  const confirm = useConfirm();
  const finished = p.jobs.filter((j) => j.status === "done" || j.status === "cancelled").slice(0, 10);
  const clear = async () => {
    if (!(await confirm({ title: `Remove ${failed.length} failed job${failed.length === 1 ? "" : "s"} from the list?`, body: "Their error messages disappear too. The recordings stay; you can start the work again from Recordings.", confirm: "Remove" }))) return;
    await api.clearFailedJobs();
    p.refresh();
  };
  return (
    <Page title="What's running" subtitle="Everything StreamForge is working on, one job at a time. Updates every second.">
      <div className="space-y-5">
        {paused && <Notice tone="warn"><span className="flex items-center gap-2"><PauseCircle size={15} /> {paused.message}</span></Notice>}
        <Card title="Working on">
          {current ? (
            <div>
              <div className="flex items-baseline justify-between gap-4">
                <div className="min-w-0">
                  <div className="truncate text-base font-medium">{current.message || (JOB_LABEL[current.kind] ?? current.kind)}</div>
                  <div className="truncate text-sm text-ink-300">{JOB_LABEL[current.kind] ?? current.kind}{p.streamTitle(current) ? ` · ${p.streamTitle(current)}` : ""}</div>
                </div>
                <div className="text-3xl font-semibold tabular-nums">{Math.round(current.progress * 100)}%</div>
              </div>
              <div className="mt-3"><ProgressBar value={current.progress} /></div>
              <div className="mt-2 text-sm text-ink-400">{timeLeft(p.secondsLeft)}{current.attempts > 1 && ` · try ${current.attempts} of ${current.max_attempts}`}</div>
            </div>
          ) : (
            <Empty title={paused ? "Waiting to resume" : "Nothing is running"}>
              {paused ? "Work continues by itself when the reason above clears." : waiting.length ? "The next job starts in a moment." : "Work starts when a recording is imported or your stream ends. Import one from Recordings."}
            </Empty>
          )}
        </Card>
        {failed.length > 0 && (
          <Card title={`Failed (${failed.length})`} right={<Button size="sm" onClick={clear}><Trash2 size={13} /> Clear failed</Button>}>
            <JobList jobs={failed} onChange={p.refresh} />
          </Card>
        )}
        {retrying.length > 0 && (
          <Card title={`Will try again (${retrying.length})`}>
            <JobList jobs={retrying} onChange={p.refresh} />
          </Card>
        )}
        <Card title={`Up next (${waiting.length})`}>
          {waiting.length > 0 ? <JobList jobs={waiting} onChange={p.refresh} /> : <Empty>Nothing waiting.</Empty>}
        </Card>
        {finished.length > 0 && <Card title="Finished recently"><JobList jobs={finished} onChange={p.refresh} /></Card>}
      </div>
    </Page>
  );
}
