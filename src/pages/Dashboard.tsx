import { AlertTriangle, CheckCircle2, Inbox, PauseCircle } from "lucide-react";
import type { ReactNode } from "react";
import type { PageId } from "../App";
import { timeLeft, useJobs } from "../components/Progress";
import { StageView } from "../components/StreamBits";
import { Badge, Button, Card, Empty, Notice, Page, ProgressBar, Stat } from "../components/ui";
import { api } from "../lib/api";
import { ago, duration } from "../lib/format";
import { JOB_LABEL, streamStage } from "../lib/stages";
import { usePoll } from "../lib/usePoll";
import { ActivityRow } from "./Activity";

type Need = { key: string; tone: "bad" | "warn" | "info"; text: string; action: string; page: PageId };

export default function Dashboard({ go }: { go: (p: PageId) => void }) {
  const { data: ov, error } = usePoll(api.overview);
  const activity = usePoll(api.activity, 4000);
  const clips = usePoll(api.clips, 6000);
  const posts = usePoll(api.posts, 6000);
  const engine = usePoll(api.engine, 10000);
  const platforms = usePoll(api.platforms, 15000);
  const p = useJobs();

  const k = ov?.kick;
  const allClips = clips.data ?? [];
  const allPosts = posts.data ?? [];
  const toReview = allClips.filter((c) => c.status === "suggested" && c.render_status === "done").length;
  const drafts = allPosts.filter((x) => x.status === "draft").length;
  const failedPosts = allPosts.filter((x) => x.status === "failed").length;
  const failedJobs = p?.failed.length ?? ov?.jobs.failed ?? 0;
  const waitingFile = (ov?.latest_stream?.status === "awaiting_file") ? 1 : 0;
  const ready = engine.data?.status.whisper_installed && engine.data.status.model_installed;

  const needs: Need[] = [
    ...(failedJobs ? [{ key: "jobs", tone: "bad" as const, text: `${failedJobs} job${failedJobs === 1 ? "" : "s"} failed. See why and try again.`, action: "Open failed jobs", page: "progress" as const }] : []),
    ...(failedPosts ? [{ key: "fposts", tone: "bad" as const, text: `${failedPosts} upload${failedPosts === 1 ? "" : "s"} failed.`, action: "Open Upload queue", page: "uploads" as const }] : []),
    ...(ov && !ready && engine.data ? [{ key: "engine", tone: "warn" as const, text: "The transcription engine is not installed, so no clips can be found.", action: "Install in Settings", page: "settings" as const }] : []),
    ...(ov && (!k?.configured || !ov.recording_dir_set) ? [{ key: "setup", tone: "warn" as const, text: `Setup incomplete: ${[!k?.configured && "connect Kick", !ov.recording_dir_set && "choose your recording folder"].filter(Boolean).join(" and ")}.`, action: "Open setup guide", page: "setup" as const }] : []),
    ...(toReview ? [{ key: "review", tone: "info" as const, text: `${toReview} clip${toReview === 1 ? " is" : "s are"} ready for you to approve or reject.`, action: "Review clips", page: "clips" as const }] : []),
    ...(drafts ? [{ key: "drafts", tone: "info" as const, text: `${drafts} draft${drafts === 1 ? " is" : "s are"} waiting. Nothing posts until you publish or schedule it.`, action: "Open Upload queue", page: "uploads" as const }] : []),
    ...(waitingFile ? [{ key: "file", tone: "info" as const, text: "Your stream ended. Waiting for OBS to save the recording in your recording folder.", action: "See recordings", page: "library" as const }] : []),
  ];
  const needsLoaded = ov && clips.data && posts.data;

  const latest = ov?.latest_stream ?? null;
  const latestStage = latest ? streamStage(latest, p?.jobs ?? [], allClips) : null;
  const current = p?.current ?? null;
  const published = allPosts.filter((x) => x.status === "published").sort((a, b) => (b.published_at ?? "").localeCompare(a.published_at ?? "")).slice(0, 3);

  const dot = (on: boolean | undefined, label: string, off: string, page: PageId): ReactNode => (
    <li key={label} className="flex items-center justify-between gap-3 py-1.5">
      <span>{label}</span>
      {on ? <Badge tone="good">Ready</Badge> : on === undefined ? <Badge>Checking</Badge> : <button onClick={() => go(page)} className="text-left text-[13px] text-amber-300 hover:underline">{off}</button>}
    </li>
  );
  const acct = (id: string) => platforms.data?.find((x) => x.id === id)?.account != null;

  return (
    <Page title="Dashboard" subtitle="What happened, what needs you, and what is running.">
      {error && <div className="mb-4"><Notice tone="bad">Could not read the app state: {error}</Notice></div>}

      <div className="grid grid-cols-2 gap-4 lg:grid-cols-4">
        <Stat
          label="Kick"
          onClick={() => go(k?.configured ? "live" : "connections")}
          value={!k?.configured ? <span className="text-ink-400">Not connected</span> : k.live ? <span className="text-kick">Live</span> : "Offline"}
          hint={k?.configured ? (k.last_error ? <span className="text-red-300">Check failed: {k.last_error}</span> : `checked ${ago(k.last_checked)}`) : "Connect it in Accounts"}
        />
        <Stat label="Recordings" onClick={() => go("library")} value={ov?.stream_count ?? "-"} hint="in the library" />
        <Stat label="Clips to review" onClick={() => go("clips")} value={clips.data ? toReview : "-"} hint={`${allClips.filter((c) => c.status === "approved").length} approved`} />
        <Stat label="Waiting to post" onClick={() => go("uploads")} value={posts.data ? drafts + allPosts.filter((x) => x.status === "scheduled").length : "-"} hint={`${drafts} draft · ${allPosts.filter((x) => x.status === "scheduled").length} scheduled`} />
      </div>

      <div className="mt-5 grid gap-5 lg:grid-cols-2">
        <Card title="Needs you">
          {!needsLoaded ? (
            <div className="text-sm text-ink-400">Checking…</div>
          ) : needs.length === 0 ? (
            <div className="flex items-center gap-3 text-sm text-emerald-300"><CheckCircle2 size={18} /> All caught up. Nothing is waiting on you.</div>
          ) : (
            <ul className="space-y-3">
              {needs.map((n) => (
                <li key={n.key} className="flex items-start gap-3">
                  {n.tone === "bad" ? <AlertTriangle size={17} className="mt-0.5 shrink-0 text-red-300" /> : <Inbox size={17} className={`mt-0.5 shrink-0 ${n.tone === "warn" ? "text-amber-300" : "text-sky-300"}`} />}
                  <span className="min-w-0 flex-1 text-sm leading-snug">{n.text}</span>
                  <Button size="sm" onClick={() => go(n.page)}>{n.action}</Button>
                </li>
              ))}
            </ul>
          )}
        </Card>

        <Card title="Running now" right={<Button size="sm" onClick={() => go("progress")}>All jobs</Button>}>
          {current ? (
            <div>
              <div className="flex items-baseline justify-between gap-3">
                <div className="min-w-0">
                  <div className="truncate font-medium">{current.message || JOB_LABEL[current.kind] || current.kind}</div>
                  <div className="truncate text-[13px] text-ink-300">{JOB_LABEL[current.kind] ?? current.kind}{p?.streamTitle(current) ? ` · ${p.streamTitle(current)}` : ""}</div>
                </div>
                <div className="text-2xl font-semibold tabular-nums">{Math.round(current.progress * 100)}%</div>
              </div>
              <div className="mt-2.5"><ProgressBar value={current.progress} /></div>
              <div className="mt-1.5 text-[13px] text-ink-400">{timeLeft(p?.secondsLeft ?? null)}{p && p.waiting.length > 0 && ` · ${p.waiting.length} more waiting`}</div>
            </div>
          ) : p?.paused ? (
            <div className="flex items-start gap-2 text-sm text-amber-300"><PauseCircle size={17} className="mt-0.5 shrink-0" /> {p.paused.message}</div>
          ) : p && p.waiting.length > 0 ? (
            <div className="text-sm text-ink-300">{p.waiting.length} job{p.waiting.length === 1 ? "" : "s"} waiting; the next one starts in a moment.</div>
          ) : (
            <Empty title="Nothing is running">Work starts when a recording is imported or your stream ends.</Empty>
          )}
        </Card>
      </div>

      <div className="mt-5 grid gap-5 lg:grid-cols-5">
        <Card title="Latest recording" className="lg:col-span-3" right={<Button size="sm" onClick={() => go("library")}>All recordings</Button>}>
          {latest && latestStage ? (
            <div className="space-y-3">
              <div className="flex flex-wrap items-center gap-2">
                <div className="min-w-0 truncate text-base font-medium">{latest.title}</div>
                {latest.source === "kick" && <Badge tone="live">Kick</Badge>}
              </div>
              <StageView stage={latestStage} eta={latestStage.running ? p?.secondsLeft ?? null : undefined} />
              <div className="flex flex-wrap gap-x-5 gap-y-1 text-[13px] text-ink-300">
                {latest.duration_s != null && <span>{duration(latest.duration_s)} long</span>}
                {latest.width && <span>{latest.width}×{latest.height} at {latest.fps} fps</span>}
                <span>added {ago(latest.created_at)}</span>
              </div>
            </div>
          ) : (
            <Empty title="No recordings yet" action={<Button variant="primary" onClick={() => go("library")}>Add a recording</Button>}>
              Save a stream from OBS into your recording folder, or import a file by hand.
            </Empty>
          )}
          {published.length > 0 && (
            <div className="mt-5">
              <div className="mb-1.5 text-sm font-medium">Recently posted</div>
              <ul className="space-y-1 text-[13px] text-ink-300">
                {published.map((x) => <li key={x.id} className="truncate">{x.platform === "youtube" ? "YouTube" : "TikTok"} · {x.title} · {ago(x.published_at)}</li>)}
              </ul>
            </div>
          )}
        </Card>

        <Card title="Is everything set up?" className="lg:col-span-2">
          <ul className="divide-y divide-ink-800 text-sm">
            {dot(k ? k.configured : undefined, "Kick connected", "Connect Kick", "connections")}
            {dot(ov ? ov.recording_dir_set : undefined, "Recording folder chosen", "Choose a folder", "settings")}
            {dot(engine.data ? !!ready : undefined, "Transcription engine", "Install it", "settings")}
            {dot(engine.data ? engine.data.claude : undefined, "Claude ratings (optional)", "Add a key for better titles", "settings")}
            {dot(platforms.data ? acct("youtube") : undefined, "YouTube account", "Connect YouTube", "connections")}
            {dot(platforms.data ? acct("tiktok") : undefined, "TikTok account", "Connect TikTok", "connections")}
          </ul>
        </Card>
      </div>

      <Card title="What happened" className="mt-5" right={<Button size="sm" onClick={() => go("activity")}>Open activity log</Button>}>
        {activity.data?.length ? (
          <ul className="divide-y divide-ink-800">
            {activity.data.slice(0, 8).map((a) => <ActivityRow key={a.id} a={a} />)}
          </ul>
        ) : (
          <Empty title="Nothing logged yet">Entries appear as StreamForge imports recordings, processes them and posts.</Empty>
        )}
      </Card>
    </Page>
  );
}
