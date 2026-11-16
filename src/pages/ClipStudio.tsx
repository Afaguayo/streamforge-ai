import { save } from "@tauri-apps/plugin-dialog";
import { openUrl, revealItemInDir } from "@tauri-apps/plugin-opener";
import { Check, Download, ExternalLink, FolderOpen, Pencil, RefreshCw, RotateCcw, Sparkles, X } from "lucide-react";
import { useMemo, useState } from "react";
import type { PageId } from "../App";
import { useJobs } from "../components/Progress";
import { StageView } from "../components/StreamBits";
import { Badge, Button, Card, Empty, Facts, Field, Notice, Page, ProgressBar, Stepper, Tabs, inputCls } from "../components/ui";
import { api, errorText, fileUrl, type Clip, type Post, type Word } from "../lib/api";
import { bytes, duration } from "../lib/format";
import { CLIP_STEPS, clipStage, payloadId, platformName, privacyLabel, streamStage } from "../lib/stages";
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

/// Same colours as the burned-in captions (render.rs).
const SPEAKER_COLOURS = ["#FFFFFF", "#4FD8FF", "#FF7AB8", "#9CFF6B", "#FFB84D", "#C9A2FF"];
const speakerColour = (sp: number) => SPEAKER_COLOURS[sp % SPEAKER_COLOURS.length];

type Turn = { sp: number; first: number; words: Word[] };

/// Consecutive words by the same speaker. `edit_clip_text` expects this same grouping.
function turnsOf(words: Word[]): Turn[] {
  const out: Turn[] = [];
  words.forEach((w, i) => {
    const last = out[out.length - 1];
    if (last && last.sp === w.sp) last.words.push(w);
    else out.push({ sp: w.sp, first: i, words: [w] });
  });
  return out;
}

/** A bleeped word is never shown, only this marker, so it can still be un-bleeped. */
const BLEEP = "[bleep]";
const masked = (_t: string) => BLEEP;

function Transcript({ c, onBleep }: { c: Clip; onBleep: (index: number) => void }) {
  const turns = turnsOf(c.words);
  const speakers = new Set(c.words.map((w) => w.sp)).size;
  return (
    <div className="mt-3 max-h-64 overflow-y-auto rounded-md bg-ink-950 p-3 text-sm leading-relaxed">
      <p className="mb-2 text-[13px] text-ink-400">Click a word to bleep or un-bleep it, then re-render. [bleep] marks a bleeped word: it is silenced and left out of captions.</p>
      {turns.map((t) => (
        <p key={t.first} className="mb-2">
          {speakers > 1 && <span className="mr-2 text-xs font-semibold" style={{ color: speakerColour(t.sp) }}>Speaker {t.sp + 1}</span>}
          {t.words.map((w, i) => (
            <button key={i} onClick={() => onBleep(t.first + i)} title={w.b ? "Un-bleep" : "Bleep"} className={`mr-1 rounded px-0.5 hover:bg-ink-800 ${w.b ? "bg-red-950" : ""}`} style={{ color: speakerColour(t.sp) }}>
              {w.b ? "[bleep]" : w.t}
            </button>
          ))}
        </p>
      ))}
    </div>
  );
}

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
  const turns = useMemo(() => turnsOf(c.words), [c.words]);
  const [texts, setTexts] = useState(() => turns.map((t) => t.words.map((w) => (w.b ? masked(w.t) : w.t)).join(" ")));
  const len = Number(end) - Number(start);
  const ok = len >= 5 && len <= 180;
  const saveEdits = async () => {
    try {
      const original = turns.map((t) => t.words.map((w) => (w.b ? masked(w.t) : w.t)).join(" "));
      if (texts.some((t, i) => t !== original[i])) await api.editClipText(c.id, texts);
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
        <Field label="Length"><div className={`py-2 text-sm ${ok ? "text-ink-100" : "text-red-300"}`}>{duration(len)} {ok ? "" : "· must be 5–180 s"}</div></Field>
      </div>
      <Field label="Title"><input className={inputCls} value={title} onChange={(e) => setTitle(e.target.value)} /></Field>
      <Field label="On-screen hook" hint="Shown for the first 3 seconds. Leave empty to use the title."><input className={inputCls} value={hook} onChange={(e) => setHook(e.target.value)} /></Field>
      {turns.length > 0 && (
        <Field label="Captions" hint="Fix what the speech engine got wrong. Each box is one speaker's turn. [bleep] is a bleeped word: keep it to keep the bleep, or type one to add a bleep.">
          <div className="space-y-2">
            {turns.map((t, i) => (
              <textarea key={t.first} rows={Math.min(4, Math.ceil(texts[i].length / 80) + 1)} className={inputCls} style={{ color: speakerColour(t.sp) }} value={texts[i]}
                onChange={(e) => setTexts(texts.map((x, j) => (j === i ? e.target.value : x)))} />
            ))}
          </div>
        </Field>
      )}
      <div className="flex gap-2">
        <Button variant="primary" disabled={!ok} onClick={saveEdits}>Save and re-render</Button>
        <Button onClick={() => onDone(null)}>Cancel</Button>
      </div>
    </div>
  );
}

const LANG: Record<string, string> = { en: "English", es: "Spanish", pt: "Portuguese", fr: "French", de: "German", it: "Italian" };
const langName = (l: string | null) => (l ? LANG[l.toLowerCase()] ?? l.toUpperCase() : null);

function ClipCard({ c, posts, refresh, notify }: { c: Clip; posts: Post[]; refresh: () => void; notify: (m: string) => void }) {
  const [editing, setEditing] = useState(false);
  const [showText, setShowText] = useState(false);
  const p = useJobs();
  const stage = clipStage(c, p?.jobs ?? [], posts);
  const runningJob = p?.jobs.find((j) => j.kind === "render" && payloadId(j, "clip") === c.id && j.status === "running");
  const mine = posts.filter((x) => x.clip_id === c.id);
  const bleeps = c.words.filter((w) => w.b).length;
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
  const download = async () => {
    const name = (c.title ?? "clip").replace(/[^\p{L}\p{N} _-]/gu, "").trim().slice(0, 60) || "clip";
    const dest = await save({ defaultPath: `${name}.mp4`, filters: [{ name: "Video", extensions: ["mp4"] }] });
    if (dest) act(() => api.downloadClip(c.id, dest), "Clip saved.");
  };
  return (
    <Card>
      <div className="flex flex-wrap gap-5 sm:flex-nowrap">
        <div className="w-44 shrink-0">
          {c.render_status === "done" && c.render_path ? (
            <video className="aspect-[9/16] w-full rounded-md bg-black" src={fileUrl(c.render_path)} poster={c.cover_path ? fileUrl(c.cover_path) : undefined} controls preload="none" />
          ) : (
            <div className="flex aspect-[9/16] w-full flex-col items-center justify-center gap-2 rounded-md bg-ink-950 p-3 text-center text-[13px] text-ink-300">
              <span className={stage.failed ? "font-medium text-red-300" : "font-medium text-ink-100"}>{stage.label}</span>
              {stage.percent != null && <div className="w-full"><ProgressBar value={stage.percent} /><div className="mt-1 text-xs tabular-nums">{Math.round(stage.percent * 100)}%</div></div>}
              {stage.detail && <span className="text-xs leading-snug text-ink-400">{stage.detail}</span>}
            </div>
          )}
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex items-start justify-between gap-4">
            <div className="min-w-0">
              <div className="truncate text-base font-medium" title={c.title ?? ""}>{c.title ?? "Untitled moment"}</div>
              <div className="mt-1 flex flex-wrap items-center gap-2 text-xs text-ink-300">
                {c.category && <Badge>{c.category}</Badge>}
                <Badge tone={c.scored_by === "claude" ? "info" : c.scored_by === "manual" ? "good" : "neutral"}>{c.scored_by === "claude" ? "Rated by Claude" : c.scored_by === "manual" ? "Picked by you" : "Scored by loudness and excitement"}</Badge>
                {c.status === "approved" && <Badge tone="good">Approved</Badge>}
                {c.status === "rejected" && <Badge>Rejected</Badge>}
              </div>
            </div>
            <div className="text-right" title="Overall score out of 100">
              <div className="text-3xl font-semibold tabular-nums">{Math.round(c.score)}</div>
              <div className="text-xs text-ink-400">score</div>
            </div>
          </div>

          <div className="mt-3 space-y-2.5">
            <Stepper steps={CLIP_STEPS} current={stage.step} failed={stage.failed} finished={stage.finished} />
            <StageView stage={stage} eta={stage.running && runningJob ? p?.eta(runningJob) ?? null : undefined} />
          </div>

          {c.reason && (
            <div className="mt-3 text-[13px] leading-snug">
              <span className="font-medium text-ink-100">Why it was picked: </span>
              <span className="text-ink-300">{c.reason}</span>
            </div>
          )}
          {c.hook && <p className="mt-1 text-[13px]"><span className="font-medium">On-screen hook: </span><span className="text-ink-300">“{c.hook}”</span></p>}

          <div className="mt-3">
            <Facts
              items={[
                ["Length", duration(c.end_s - c.start_s)],
                ["Taken from", `${duration(c.start_s)} to ${duration(c.end_s)} of the recording`],
                ["Captions", c.words.length ? `${langName(c.language) ?? "Language unknown"}, ${c.words.length} words` : "No word timings"],
                ["Bleeps", c.words.length ? (bleeps ? `${bleeps} word${bleeps === 1 ? "" : "s"} bleeped` : "None") : null],
                ["File size", c.render_size != null ? bytes(c.render_size) : null],
              ]}
            />
            {c.render_path && (
              <div className="mt-2 flex flex-wrap items-center gap-2 text-[13px]">
                <span className="min-w-0 break-all text-ink-400" title={c.render_path}>{c.render_path}</span>
                <Button size="sm" onClick={() => act(() => revealItemInDir(c.render_path!))}><FolderOpen size={13} /> Open folder</Button>
              </div>
            )}
          </div>

          {mine.length > 0 && (
            <ul className="mt-3 space-y-1 text-[13px]">
              {mine.map((x) => (
                <li key={x.id} className="flex flex-wrap items-center gap-2">
                  <Badge tone={x.status === "published" ? "good" : x.status === "failed" ? "bad" : x.status === "uploading" ? "info" : "neutral"}>{platformName(x.platform)}: {x.status === "published" ? "live" : x.status}</Badge>
                  <span className="text-ink-300">{privacyLabel(x.platform, x.privacy)}</span>
                  {x.status === "published" && x.url && <button className="inline-flex items-center gap-1 text-sky-300 hover:underline" onClick={() => openUrl(x.url!)}>Open link <ExternalLink size={12} /></button>}
                  {x.status === "failed" && x.error && <span className="text-red-300">{x.error}</span>}
                </li>
              ))}
            </ul>
          )}

          <div className="mt-4 flex flex-wrap gap-2">
            {c.status !== "approved" && (
              <Button variant="primary" onClick={() => act(() => api.approveClip(c.id), "Approved. Drafts are in the Upload queue.")}><Check size={14} /> Approve</Button>
            )}
            {c.status !== "rejected" ? (
              <Button onClick={() => act(() => api.rejectClip(c.id), "Rejected. You can restore it from the Rejected tab.")}><X size={14} /> Reject</Button>
            ) : (
              <Button onClick={() => act(() => api.restoreClip(c.id))}><RotateCcw size={14} /> Restore</Button>
            )}
            <Button onClick={() => setEditing(!editing)}><Pencil size={14} /> Edit</Button>
            <Button variant={stage.failed ? "primary" : "ghost"} disabled={stage.running} onClick={() => act(() => api.renderClip(c.id), "Render queued.")}>
              <RefreshCw size={14} /> {stage.failed ? "Try again" : c.render_status === "done" ? "Re-render" : "Render"}
            </Button>
            {c.render_status === "done" && c.render_path && <Button onClick={download}><Download size={14} /> Download</Button>}
            {c.transcript && <Button onClick={() => setShowText(!showText)}>{showText ? "Hide" : "Show"} transcript</Button>}
          </div>
          <details className="mt-3 text-[13px]">
            <summary className="cursor-pointer text-ink-300 hover:text-ink-100">Score breakdown</summary>
            <div className="mt-2"><ScoreBars c={c} /></div>
          </details>
          {editing && <EditForm c={c} onDone={(m) => { setEditing(false); if (m) notify(m); refresh(); }} />}
          {showText && (c.words.length > 0
            ? <Transcript c={c} onBleep={(i) => act(() => api.toggleBleep(c.id, i), "Changed. Re-render the clip to hear it.")} />
            : <p className="mt-3 max-h-48 overflow-y-auto rounded-md bg-ink-950 p-3 text-sm leading-relaxed text-ink-300">{c.transcript}</p>)}
        </div>
      </div>
    </Card>
  );
}

export default function ClipStudio({ go }: { go: (p: PageId) => void }) {
  const { data: clips, refresh } = usePoll(api.clips, 4000);
  const { data: streams } = usePoll(api.streams, 8000);
  const { data: engine } = usePoll(api.engine, 8000);
  const { data: posts } = usePoll(api.posts, 5000);
  const p = useJobs();
  const [streamId, setStreamId] = useState<number | null>(null);
  const [tab, setTab] = useState<Clip["status"]>("suggested");
  const [msg, setMsg] = useState<string | null>(null);

  const withClips = useMemo(() => [...new Set((clips ?? []).map((c) => c.stream_id))], [clips]);
  const current = streamId ?? withClips[0] ?? null;
  const shown = (clips ?? []).filter((c) => c.stream_id === current && c.status === tab);
  const ready = engine?.status.whisper_installed && engine.status.model_installed;
  const unprocessed = (streams ?? []).filter((s) => s.status === "probed");
  const currentStream = (streams ?? []).find((s) => s.id === current) ?? null;
  const streamState = currentStream ? streamStage(currentStream, p?.jobs ?? [], clips ?? []) : null;
  const countOf = (t: Clip["status"]) => (clips ?? []).filter((c) => c.stream_id === current && c.status === t).length;

  return (
    <Page title="AI clips" subtitle="The best 60–120 s moments from each stream, ranked. Approve the ones you want to post.">
      {msg && <div className="mb-4"><Notice tone="info">{msg}</Notice></div>}
      {engine && !ready && (
        <div className="mb-4">
          <Notice tone="warn">
            <div className="flex items-center justify-between gap-4">
              <span>Install the transcription engine first. StreamForge needs it to understand what is said in your streams.</span>
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
              <span>{unprocessed.length} recording(s) have not been searched for clips yet.</span>
              <Button onClick={async () => { for (const s of unprocessed) await api.processStream(s.id); setMsg("Finding clips; this runs in the background."); }}>
                <Sparkles size={14} /> Find clips
              </Button>
            </div>
          </Notice>
        </div>
      )}

      <div className="mb-4 flex flex-wrap items-center gap-3">
        <select aria-label="Recording" className={`${inputCls} w-80 max-w-full`} value={current ?? ""} onChange={(e) => setStreamId(Number(e.target.value))}>
          {withClips.length === 0 && <option value="">No recordings with clips yet</option>}
          {withClips.map((id) => <option key={id} value={id}>{(clips ?? []).find((c) => c.stream_id === id)?.stream_title}</option>)}
        </select>
        <Tabs<Clip["status"]>
          value={tab}
          onChange={setTab}
          tabs={[
            { id: "suggested", label: "To review", count: countOf("suggested") },
            { id: "approved", label: "Approved", count: countOf("approved") },
            { id: "rejected", label: "Rejected", count: countOf("rejected") },
          ]}
        />
        {current && <Button onClick={async () => { await api.processStream(current); setMsg("Re-analysing this recording."); }}><RefreshCw size={14} /> Re-analyse</Button>}
      </div>
      {streamState && currentStream && (
        <div className="mb-4 rounded-lg bg-ink-900 px-5 py-3">
          <div className="mb-1.5 text-[13px] text-ink-300">This recording: <span className="text-ink-100">{currentStream.title}</span></div>
          <StageView stage={streamState} />
        </div>
      )}

      <div className="space-y-4">
        {shown.length === 0 ? (
          withClips.length === 0 ? (
            <Empty title="No clips yet" action={<Button onClick={() => go("library")}>Go to Recordings</Button>}>
              Clips appear after a recording is transcribed and analysed. Open Recordings and press Find clips, or cut one yourself.
            </Empty>
          ) : (
            <Empty title={tab === "suggested" ? "Nothing left to review" : `No ${tab} clips here`}>
              {tab === "suggested" ? "You have been through every suggestion for this recording. Check the Approved tab, or pick another recording." : tab === "approved" ? "Approve a clip from the To review tab and its drafts appear in the Upload queue." : "Rejected clips are kept here so you can restore them."}
            </Empty>
          )
        ) : (
          shown.map((c) => <ClipCard key={c.id} c={c} posts={posts ?? []} refresh={refresh} notify={setMsg} />)
        )}
      </div>
    </Page>
  );
}
