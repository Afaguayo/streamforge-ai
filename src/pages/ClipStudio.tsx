import { Check, Pencil, RefreshCw, RotateCcw, Sparkles, X } from "lucide-react";
import { useMemo, useState } from "react";
import type { PageId } from "../App";
import { Badge, Button, Card, Empty, Field, Notice, Page, inputCls } from "../components/ui";
import { api, errorText, fileUrl, type Clip } from "../lib/api";
import { duration } from "../lib/format";
import { usePoll } from "../lib/usePoll";

const DIMENSIONS: [keyof Clip["scores"], string][] = [
  ["hook", "Hook"],
  ["entertainment", "Entertainment"],
  ["emotion", "Emotion"],
  ["context", "Context"],
  ["replay", "Replay"],
  ["shareability", "Shareability"],
  ["trend", "Trend fit"],
];

function ScoreBars({ c }: { c: Clip }) {
  return (
    <div className="grid grid-cols-2 gap-x-5 gap-y-1.5">
      {DIMENSIONS.map(([k, label]) => (
        <div key={k} className="text-xs">
          <div className="flex justify-between text-ink-300">
            <span>{label}</span>
            <span className="tabular-nums">{Math.round(c.scores?.[k] ?? 0)}</span>
          </div>
          <div className="mt-0.5 h-1 rounded bg-ink-800">
            <div className="h-full rounded bg-sky-400" style={{ width: `${c.scores?.[k] ?? 0}%` }} />
          </div>
        </div>
      ))}
    </div>
  );
}

function EditForm({ c, onDone }: { c: Clip; onDone: (msg: string | null) => void }) {
  const [start, setStart] = useState(c.start_s.toFixed(1));
  const [end, setEnd] = useState(c.end_s.toFixed(1));
  const [title, setTitle] = useState(c.title ?? "");
  const [hook, setHook] = useState(c.hook ?? "");
  const len = Number(end) - Number(start);
  const ok = len >= 60 && len <= 120;
  const save = async () => {
    try {
      await api.editClip(c.id, Number(start), Number(end), title, hook);
      onDone("Saved; re-rendering the clip.");
    } catch (e) {
      onDone(errorText(e));
    }
  };
  return (
    <div className="mt-3 space-y-3 rounded-lg bg-ink-950 p-4">
      <div className="grid grid-cols-3 gap-3">
        <Field label="Start (s)"><input className={inputCls} value={start} onChange={(e) => setStart(e.target.value)} /></Field>
        <Field label="End (s)"><input className={inputCls} value={end} onChange={(e) => setEnd(e.target.value)} /></Field>
        <Field label="Length"><div className={`py-2 text-sm ${ok ? "text-ink-100" : "text-red-300"}`}>{duration(len)} {ok ? "" : "· must be 60–120 s"}</div></Field>
      </div>
      <Field label="Title"><input className={inputCls} value={title} onChange={(e) => setTitle(e.target.value)} /></Field>
      <Field label="On-screen hook" hint="Shown for the first 3 seconds. Leave empty to use the title."><input className={inputCls} value={hook} onChange={(e) => setHook(e.target.value)} /></Field>
      <div className="flex gap-2">
        <Button variant="primary" disabled={!ok} onClick={save}>Save and re-render</Button>
        <Button onClick={() => onDone(null)}>Cancel</Button>
      </div>
    </div>
  );
}

function ClipCard({ c, refresh, notify }: { c: Clip; refresh: () => void; notify: (m: string) => void }) {
  const [editing, setEditing] = useState(false);
  const [showText, setShowText] = useState(false);
  const act = async (f: () => Promise<unknown>, ok?: string) => {
    try {
      const r = await f();
      const notes = Array.isArray(r) && r.length ? ` ${r.join(" ")}` : "";
      if (ok || notes) notify((ok ?? "") + notes);
      refresh();
    } catch (e) {
      notify(errorText(e));
    }
  };
  return (
    <Card>
      <div className="flex gap-5">
        <div className="w-44 shrink-0">
          {c.render_status === "done" && c.render_path ? (
            <video className="aspect-[9/16] w-full rounded-lg bg-black" src={fileUrl(c.render_path)} poster={c.cover_path ? fileUrl(c.cover_path) : undefined} controls preload="none" />
          ) : (
            <div className="flex aspect-[9/16] w-full items-center justify-center rounded-lg border border-dashed border-ink-700 p-3 text-center text-xs text-ink-500">
              {c.render_status === "queued" ? "Rendering…" : c.render_status === "error" ? <span className="text-red-300">Render failed</span> : "Not rendered yet"}
            </div>
          )}
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex items-start justify-between gap-4">
            <div className="min-w-0">
              <div className="truncate text-base font-medium" title={c.title ?? ""}>{c.title ?? "Untitled moment"}</div>
              <div className="mt-1 flex flex-wrap items-center gap-2 text-xs text-ink-300">
                <span>{duration(c.start_s)} → {duration(c.end_s)} · {duration(c.end_s - c.start_s)}</span>
                {c.category && <Badge>{c.category}</Badge>}
                <Badge tone={c.scored_by === "claude" ? "info" : "neutral"}>{c.scored_by === "claude" ? "rated by Claude" : "heuristic score"}</Badge>
                {c.posts.map(([p, s]) => <Badge key={p} tone={s === "published" ? "good" : "neutral"}>{p}: {s}</Badge>)}
              </div>
            </div>
            <div className="text-right">
              <div className="text-3xl font-semibold tabular-nums">{Math.round(c.score)}</div>
              <div className="text-[11px] uppercase tracking-wider text-ink-500">score</div>
            </div>
          </div>
          {c.reason && <p className="mt-2 text-sm text-ink-300">{c.reason}</p>}
          {c.hook && <p className="mt-1 text-sm">Hook: <span className="text-ink-100">“{c.hook}”</span></p>}
          <div className="mt-3"><ScoreBars c={c} /></div>
          {c.error && <div className="mt-2 text-sm text-red-300">{c.error}</div>}
          <div className="mt-4 flex flex-wrap gap-2">
            {c.status !== "approved" && (
              <Button variant="primary" onClick={() => act(() => api.approveClip(c.id), "Approved. Drafts are in the Upload Queue.")}><Check size={14} /> Approve</Button>
            )}
            {c.status !== "rejected" ? (
              <Button onClick={() => act(() => api.rejectClip(c.id))}><X size={14} /> Reject</Button>
            ) : (
              <Button onClick={() => act(() => api.restoreClip(c.id))}><RotateCcw size={14} /> Restore</Button>
            )}
            <Button onClick={() => setEditing(!editing)}><Pencil size={14} /> Edit</Button>
            <Button onClick={() => act(() => api.renderClip(c.id), "Re-render queued.")}><RefreshCw size={14} /> {c.render_status === "done" ? "Re-render" : "Render"}</Button>
            {c.transcript && <Button onClick={() => setShowText(!showText)}>{showText ? "Hide" : "Show"} transcript</Button>}
          </div>
          {editing && <EditForm c={c} onDone={(m) => { setEditing(false); if (m) notify(m); refresh(); }} />}
          {showText && <p className="mt-3 max-h-48 overflow-y-auto rounded-lg bg-ink-950 p-3 text-sm leading-relaxed text-ink-300">{c.transcript}</p>}
        </div>
      </div>
    </Card>
  );
}

export default function ClipStudio({ go }: { go: (p: PageId) => void }) {
  const { data: clips, refresh } = usePoll(api.clips, 4000);
  const { data: streams } = usePoll(api.streams, 8000);
  const { data: engine } = usePoll(api.engine, 8000);
  const [streamId, setStreamId] = useState<number | null>(null);
  const [tab, setTab] = useState<Clip["status"]>("suggested");
  const [msg, setMsg] = useState<string | null>(null);

  const withClips = useMemo(() => [...new Set((clips ?? []).map((c) => c.stream_id))], [clips]);
  const current = streamId ?? withClips[0] ?? null;
  const shown = (clips ?? []).filter((c) => c.stream_id === current && c.status === tab);
  const ready = engine?.status.whisper_installed && engine.status.model_installed;
  const unprocessed = (streams ?? []).filter((s) => s.status === "probed");

  return (
    <Page title="AI Clip Studio" subtitle="The best 60–120 s moments from each stream, ranked. Approve the ones you want to post.">
      {msg && <div className="mb-4"><Notice tone="info">{msg}</Notice></div>}
      {engine && !ready && (
        <div className="mb-4">
          <Notice tone="warn">
            <div className="flex items-center justify-between gap-4">
              <span>Install the transcription engine first. StreamForge needs it to understand what's said in your streams.</span>
              <Button onClick={() => go("settings")}>Open Settings</Button>
            </div>
          </Notice>
        </div>
      )}
      {engine && ready && !engine.claude && (
        <div className="mb-4"><Notice>Clips are ranked by loudness, laughter, excitement words and scene changes. Add a Claude API key in Settings for transcript-based ratings, titles and hooks.</Notice></div>
      )}
      {unprocessed.length > 0 && ready && (
        <div className="mb-4">
          <Notice>
            <div className="flex items-center justify-between gap-4">
              <span>{unprocessed.length} recording(s) haven't been searched for clips yet.</span>
              <Button onClick={async () => { for (const s of unprocessed) await api.processStream(s.id); setMsg("Finding clips; this runs in the background."); }}>
                <Sparkles size={14} /> Find clips
              </Button>
            </div>
          </Notice>
        </div>
      )}

      <div className="mb-4 flex items-center gap-3">
        <select className={`${inputCls} w-80`} value={current ?? ""} onChange={(e) => setStreamId(Number(e.target.value))}>
          {withClips.length === 0 && <option value="">No recordings with clips yet</option>}
          {withClips.map((id) => <option key={id} value={id}>{(clips ?? []).find((c) => c.stream_id === id)?.stream_title}</option>)}
        </select>
        <div className="flex rounded-lg border border-ink-700 p-0.5 text-sm">
          {(["suggested", "approved", "rejected"] as const).map((t) => (
            <button key={t} onClick={() => setTab(t)} className={`rounded-md px-3 py-1.5 capitalize ${tab === t ? "bg-ink-800 text-ink-100" : "text-ink-300"}`}>
              {t} ({(clips ?? []).filter((c) => c.stream_id === current && c.status === t).length})
            </button>
          ))}
        </div>
        {current && <Button onClick={async () => { await api.processStream(current); setMsg("Re-analysing this recording."); }}><RefreshCw size={14} /> Re-analyse</Button>}
      </div>

      <div className="space-y-4">
        {shown.length === 0 ? (
          <Empty>{withClips.length === 0 ? "No clips yet. They appear here after a recording is transcribed and analysed." : `No ${tab} clips for this recording.`}</Empty>
        ) : (
          shown.map((c) => <ClipCard key={c.id} c={c} refresh={refresh} notify={setMsg} />)
        )}
      </div>
    </Page>
  );
}
