import { open } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { ChevronDown, Download, FolderOpen, Plus, RefreshCw, RotateCcw, Scissors, Sparkles, Youtube } from "lucide-react";
import { PICK_STREAM_KEY } from "./ClipPicker";
import { useState } from "react";
import type { PageId } from "../App";
import { useJobs } from "../components/Progress";
import { StageView } from "../components/StreamBits";
import { Badge, Button, Card, Empty, Facts, Notice, Page, Stepper, inputCls } from "../components/ui";
import { api, errorText, type Clip, type KickVod, type Stream } from "../lib/api";
import { STREAM_STEPS, streamStage } from "../lib/stages";
import { ago, bytes, duration } from "../lib/format";
import { usePoll } from "../lib/usePoll";

function KickVods({ onMsg }: { onMsg: (m: { tone: "good" | "bad"; text: string }) => void }) {
  const [vods, setVods] = useState<KickVod[] | null>(null);
  const [link, setLink] = useState("");
  const [loading, setLoading] = useState(false);
  const load = async () => {
    setLoading(true);
    try {
      setVods(await api.kickVods());
    } catch (e) {
      onMsg({ tone: "bad", text: errorText(e) });
    }
    setLoading(false);
  };
  const add = async (f: () => Promise<unknown>) => {
    try {
      await f();
      onMsg({ tone: "good", text: "Downloading from Kick; clips are made automatically when it finishes." });
      setLink("");
      if (vods) await load();
    } catch (e) {
      onMsg({ tone: "bad", text: errorText(e) });
    }
  };
  return (
    <Card title="Past Kick VODs" right={<Button disabled={loading} onClick={load}><RefreshCw size={14} /> {vods ? "Refresh" : "Load my VODs"}</Button>}>
      <div className="flex gap-2">
        <input className={inputCls} value={link} onChange={(e) => setLink(e.target.value)} placeholder="Or paste a kick.com video link" />
        <Button variant="primary" disabled={!link.trim()} onClick={() => add(() => api.importKickLink(link))}><Download size={14} /> Add</Button>
      </div>
      {!vods && <p className="mt-3 text-[13px] text-ink-400">Load your past broadcasts to download one and make clips from it. Downloads run in the background.</p>}
      {vods?.length === 0 && <div className="mt-3"><Empty title="No past broadcasts">Kick has none for this channel yet.</Empty></div>}
      {vods && vods.length > 0 && (
        <div className="mt-3 divide-y divide-ink-800">
          {vods.map((v) => (
            <div key={v.id} className="flex items-center gap-4 py-2.5 text-sm">
              {v.thumbnail && <img src={v.thumbnail} className="h-12 w-20 shrink-0 rounded object-cover" />}
              <div className="min-w-0 flex-1">
                <div className="truncate font-medium" title={v.title}>{v.title}</div>
                <div className="text-[13px] text-ink-400">{new Date(v.started_at).toLocaleString()} · {duration(v.duration_s)}</div>
              </div>
              {v.stream_id ? <Badge tone="good">In library</Badge> : <Button onClick={() => add(() => api.importKickVod(v))}><Download size={14} /> Download</Button>}
            </div>
          ))}
        </div>
      )}
    </Card>
  );
}

function StreamCard({ s, clips, run, setMsg, go }: { s: Stream; clips: Clip[]; run: (f: () => Promise<unknown>) => void; setMsg: (m: { tone: "good" | "bad"; text: string }) => void; go: (p: PageId) => void }) {
  const p = useJobs();
  const [open, setOpen] = useState(false);
  const stage = streamStage(s, p?.jobs ?? [], clips);
  const mine = clips.filter((c) => c.stream_id === s.id);
  const rendered = mine.filter((c) => c.render_status === "done").length;
  const runningJob = p?.jobs.find((j) => j.stream_id === s.id && j.status === "running");
  const retry = () =>
    run(async () => {
      if (stage.retryJobId != null) await api.retryJob(stage.retryJobId);
      else if (s.file_path) await api.reprobe(s.id);
      setMsg({ tone: "good", text: "Trying again. Progress is shown on this card." });
    });
  const kickLen = s.kick_started && s.kick_ended ? (Date.parse(s.kick_ended) - Date.parse(s.kick_started)) / 1000 : null;
  return (
    <Card>
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            <h3 className="truncate text-base font-medium" title={s.title}>{s.title}</h3>
            {s.source === "kick" && <Badge tone="live">Kick</Badge>}
          </div>
          <div className="mt-1 text-[13px] text-ink-300">
            {[s.duration_s != null && `${duration(s.duration_s)} long`, s.width ? `${s.width}×${s.height}` : null, s.file_size != null ? bytes(s.file_size) : null, `added ${ago(s.created_at)}`].filter(Boolean).join(" · ")}
          </div>
        </div>
        <div className="flex flex-wrap gap-2">
          {stage.failed && <Button size="sm" variant="primary" onClick={retry}><RotateCcw size={13} /> Try again</Button>}
          {s.duration_s != null && (
            <Button size="sm" onClick={() => run(async () => { await api.processStream(s.id); setMsg({ tone: "good", text: "Finding clips. Progress is shown on this card." }); })}>
              <Sparkles size={13} /> {mine.length ? "Find clips again" : "Find clips"}
            </Button>
          )}
          {s.duration_s != null && s.file_path && (
            <Button size="sm" onClick={() => { try { sessionStorage.setItem(PICK_STREAM_KEY, String(s.id)); } catch { /* opens on the first recording */ } go("picker"); }}>
              <Scissors size={13} /> Cut your own
            </Button>
          )}
          {s.duration_s != null && (
            <Button size="sm" onClick={() => run(async () => { await api.draftFullVideo(s.id); setMsg({ tone: "good", text: "Full-stream draft added to the Upload queue." }); })}>
              <Youtube size={13} /> Draft full video
            </Button>
          )}
        </div>
      </div>

      <div className="mt-4 space-y-3">
        <Stepper steps={STREAM_STEPS} current={stage.step} failed={stage.failed} finished={stage.finished} />
        <StageView stage={stage} eta={stage.running && stage.percent != null && runningJob ? p?.eta(runningJob) ?? null : undefined} />
      </div>

      {mine.length > 0 && (
        <button onClick={() => go("clips")} className="mt-3 text-left text-[13px] text-sky-300 hover:underline">
          {mine.length} clips found · {rendered} rendered · {mine.filter((c) => c.status === "approved").length} approved. Open them
        </button>
      )}

      <button onClick={() => setOpen(!open)} aria-expanded={open} className="mt-3 flex items-center gap-1 text-[13px] text-ink-300 hover:text-ink-100">
        <ChevronDown size={14} className={open ? "rotate-180" : ""} /> {open ? "Hide details" : "Show details"}
      </button>
      {open && (
        <div className="mt-3 space-y-3 rounded-md bg-ink-950/60 p-4">
          <Facts
            items={[
              ["Length", duration(s.duration_s)],
              ["Video", s.width ? `${s.width}×${s.height}, ${s.video_codec ?? "?"}, ${s.fps ?? "?"} fps` : null],
              ["Audio", s.audio_codec],
              ["File size", s.file_size != null ? bytes(s.file_size) : null],
              ["Source", s.source === "kick" ? "Kick stream" : "Imported file"],
              ["Kick went live", s.kick_started ? new Date(s.kick_started).toLocaleString() : null],
              ["Kick stream length", kickLen != null ? duration(kickLen) : null],
              ["Added", new Date(s.created_at).toLocaleString()],
            ]}
          />
          {s.file_path ? (
            <div className="flex flex-wrap items-center gap-2 text-[13px]">
              <span className="min-w-0 break-all text-ink-300" title={s.file_path}>{s.file_path}</span>
              <Button size="sm" onClick={() => run(() => revealItemInDir(s.file_path!))}><FolderOpen size={13} /> Open folder</Button>
              <Button size="sm" onClick={() => run(async () => { await api.reprobe(s.id); setMsg({ tone: "good", text: "Reading the file details again." }); })}><RefreshCw size={13} /> Re-read file info</Button>
            </div>
          ) : (
            <div className="text-[13px] text-ink-400">There is no file on disk for this recording yet.</div>
          )}
        </div>
      )}
    </Card>
  );
}

export default function Library({ go }: { go: (p: PageId) => void }) {
  const { data: streams, refresh } = usePoll(api.streams, 3000);
  const { data: clips } = usePoll(api.clips, 6000);
  const [msg, setMsg] = useState<{ tone: "good" | "bad"; text: string } | null>(null);

  const importFiles = async () => {
    const picked = await open({ multiple: true, filters: [{ name: "Video", extensions: ["mp4", "mkv", "mov", "flv", "ts", "webm"] }] });
    if (!picked) return;
    const paths = Array.isArray(picked) ? picked : [picked];
    try {
      const ids = await api.importFiles(paths);
      setMsg({ tone: "good", text: `Imported ${ids.length} recording(s); reading metadata now.` });
      refresh();
    } catch (e) {
      setMsg({ tone: "bad", text: errorText(e) });
    }
  };

  const run = async (f: () => Promise<unknown>) => {
    try {
      await f();
      refresh();
    } catch (e) {
      setMsg({ tone: "bad", text: errorText(e) });
    }
  };

  return (
    <Page
      title="Recordings"
      subtitle="Every stream StreamForge knows about and exactly where each one is in the pipeline. New files in your recording folder are added automatically."
      actions={
        <>
          <Button onClick={() => go("settings")}>
            <FolderOpen size={15} /> Recording folder
          </Button>
          <Button variant="primary" onClick={importFiles}>
            <Plus size={15} /> Import recording
          </Button>
        </>
      }
    >
      {msg && (
        <div className="mb-4">
          <Notice tone={msg.tone}>{msg.text}</Notice>
        </div>
      )}
      <div className="mb-4"><KickVods onMsg={setMsg} /></div>
      {!streams?.length ? (
        <Card>
          <Empty title="No recordings yet" action={<Button variant="primary" onClick={importFiles}><Plus size={15} /> Import recording</Button>}>
            Set the folder OBS saves to and new streams appear by themselves, or import a video file now.
          </Empty>
        </Card>
      ) : (
        <div className="space-y-4">
          {streams.map((s) => <StreamCard key={s.id} s={s} clips={clips ?? []} run={run} setMsg={setMsg} go={go} />)}
        </div>
      )}
    </Page>
  );
}
