import type { PageId } from "../App";
import { JobList, StreamStatus } from "../components/StreamBits";
import { Badge, Button, Card, Empty, Notice, Page, Stat } from "../components/ui";
import { api } from "../lib/api";
import { ago, duration } from "../lib/format";
import { usePoll } from "../lib/usePoll";

export default function Dashboard({ go }: { go: (p: PageId) => void }) {
  const { data: ov, error } = usePoll(api.overview);
  const jobs = usePoll(api.jobs);
  const activity = usePoll(api.activity, 4000);

  const k = ov?.kick;
  return (
    <Page title="Dashboard" subtitle="What StreamForge is doing right now.">
      {error && <Notice tone="bad">{error}</Notice>}

      {ov && (!k?.configured || !ov.recording_dir_set) && (
        <div className="mb-5">
          <Notice tone="warn">
            <div className="flex items-center justify-between gap-4">
              <span>
                Setup incomplete:{" "}
                {[!k?.configured && "connect Kick", !ov.recording_dir_set && "choose your recording folder"].filter(Boolean).join(" and ")}.
              </span>
              <Button onClick={() => go(!k?.configured ? "connections" : "settings")}>Finish setup</Button>
            </div>
          </Notice>
        </div>
      )}

      <div className="grid grid-cols-4 gap-4">
        <Stat
          label="Kick"
          value={!k?.configured ? <span className="text-ink-500">Not connected</span> : k.live ? <span className="text-kick">Live</span> : "Offline"}
          hint={k?.configured ? (k.last_error ? <span className="text-red-300">Check failed</span> : `checked ${ago(k.last_checked)}`) : "Account Connections"}
        />
        <Stat label="Recordings" value={ov?.stream_count ?? "—"} hint="in the library" />
        <Stat label="Processing" value={ov ? ov.jobs.running + ov.jobs.queued : "—"} hint={ov ? `${ov.jobs.running} running · ${ov.jobs.queued} queued` : ""} />
        <Stat label="Failed jobs" value={ov?.jobs.failed ?? "—"} hint="retry from the queue below" />
      </div>

      <div className="mt-5 grid grid-cols-5 gap-5">
        <Card title="Latest recording" className="col-span-3" right={<Button onClick={() => go("library")}>Library</Button>}>
          {ov?.latest_stream ? (
            <div>
              <div className="flex items-center gap-3">
                <div className="truncate text-base font-medium">{ov.latest_stream.title}</div>
                <StreamStatus s={ov.latest_stream.status} />
                {ov.latest_stream.source === "kick" && <Badge tone="live">Kick</Badge>}
              </div>
              <div className="mt-2 flex gap-5 text-sm text-ink-300">
                <span>{duration(ov.latest_stream.duration_s)}</span>
                {ov.latest_stream.width && <span>{ov.latest_stream.width}×{ov.latest_stream.height} @ {ov.latest_stream.fps}fps</span>}
                <span>added {ago(ov.latest_stream.created_at)}</span>
              </div>
            </div>
          ) : (
            <Empty>No recordings yet.</Empty>
          )}
        </Card>
        <Card title="Pipeline" className="col-span-2">
          <ul className="space-y-1.5 text-sm">
            {[
              ["Ingest + metadata", true],
              ["Transcription (Whisper)", false],
              ["Moment detection + scoring", false],
              ["Clip editing + subtitles", false],
              ["Thumbnails + metadata", false],
              ["Publishing (review first)", false],
            ].map(([label, ready]) => (
              <li key={label as string} className="flex items-center justify-between">
                <span className={ready ? "" : "text-ink-500"}>{label}</span>
                {ready ? <Badge tone="good">Active</Badge> : <Badge>Not built yet</Badge>}
              </li>
            ))}
          </ul>
        </Card>
      </div>

      <div className="mt-5 grid grid-cols-5 gap-5">
        <Card title="Processing queue" className="col-span-3">
          <JobList jobs={(jobs.data ?? []).slice(0, 12)} onChange={jobs.refresh} />
        </Card>
        <Card title="Activity" className="col-span-2">
          {activity.data?.length ? (
            <ul className="max-h-96 space-y-2 overflow-y-auto pr-1 text-sm">
              {activity.data.map((a) => (
                <li key={a.id} className="leading-snug">
                  <span className={a.level === "error" ? "text-red-300" : a.level === "warn" ? "text-amber-300" : "text-ink-100"}>{a.message}</span>
                  <div className="text-xs text-ink-500">
                    {a.source} · {ago(a.ts)}
                  </div>
                </li>
              ))}
            </ul>
          ) : (
            <Empty>Nothing yet.</Empty>
          )}
        </Card>
      </div>
    </Page>
  );
}
