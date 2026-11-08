import { Plus, Search, Scissors } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import type { PageId } from "../App";
import { Button, Card, Empty, Field, Notice, Page, inputCls } from "../components/ui";
import { api, errorText, fileUrl, type Line } from "../lib/api";
import { duration } from "../lib/format";
import { usePoll } from "../lib/usePoll";

/** Set by the Video Library's "Pick clips" button, read once when this page opens. */
export const PICK_STREAM_KEY = "picker.stream";

const MIN = 5;
const MAX = 180;

export default function ClipPicker({ go }: { go: (p: PageId) => void }) {
  const { data: streams } = usePoll(api.streams, 10000);
  const { data: clips, refresh: refreshClips } = usePoll(api.clips, 5000);
  const ready = (streams ?? []).filter((s) => s.file_path && s.duration_s);
  const [sid, setSid] = useState<number | null>(() => {
    try {
      const v = sessionStorage.getItem(PICK_STREAM_KEY);
      sessionStorage.removeItem(PICK_STREAM_KEY);
      return v ? Number(v) : null;
    } catch {
      return null;
    }
  });
  const stream = ready.find((s) => s.id === sid) ?? ready[0] ?? null;
  const [lines, setLines] = useState<Line[] | null>(null);
  const [query, setQuery] = useState("");
  const [now, setNow] = useState(0);
  const [inAt, setIn] = useState<number | null>(null);
  const [outAt, setOut] = useState<number | null>(null);
  const [title, setTitle] = useState("");
  const [msg, setMsg] = useState<{ text: string; bad?: boolean } | null>(null);
  const video = useRef<HTMLVideoElement>(null);
  const list = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!stream) return;
    setLines(null);
    setIn(null);
    setOut(null);
    api.streamTranscript(stream.id).then(setLines).catch((e) => setMsg({ text: errorText(e), bad: true }));
  }, [stream?.id]);

  const seek = (t: number) => {
    const v = video.current;
    if (!v) return;
    v.currentTime = Math.max(0, Math.min(t, stream?.duration_s ?? t));
  };

  // Keyboard: Space play/pause, J/L ±5 s, I/O mark in and out. Ignored while typing.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const tag = (e.target as HTMLElement)?.tagName;
      if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return;
      const v = video.current;
      if (!v) return;
      const k = e.key.toLowerCase();
      if (k === " ") { e.preventDefault(); v.paused ? v.play() : v.pause(); }
      else if (k === "j") seek(v.currentTime - 5);
      else if (k === "l") seek(v.currentTime + 5);
      else if (k === "i") setIn(v.currentTime);
      else if (k === "o") setOut(v.currentTime);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const shown = useMemo(() => {
    const q = query.trim().toLowerCase();
    return (lines ?? []).filter((l) => !q || l.text.toLowerCase().includes(q));
  }, [lines, query]);

  // Keeps the line being spoken in view while nothing is searched.
  const currentLine = useMemo(() => (lines ?? []).findIndex((l) => now >= l.s && now < l.e + 1.5), [lines, now]);
  useEffect(() => {
    if (query || currentLine < 0) return;
    list.current?.querySelector(`[data-i="${currentLine}"]`)?.scrollIntoView({ block: "nearest" });
  }, [currentLine, query]);

  const len = inAt != null && outAt != null ? outAt - inAt : null;
  const ok = len != null && len >= MIN && len <= MAX;
  const streamClips = (clips ?? []).filter((c) => c.stream_id === stream?.id && c.status !== "rejected");
  const total = stream?.duration_s ?? 1;

  const create = async () => {
    if (!stream || inAt == null || outAt == null) return;
    try {
      await api.createClip(stream.id, inAt, outAt, title.trim());
      setMsg({ text: "Clip created and rendering. It appears in AI Clip Studio, already approved." });
      setIn(null);
      setOut(null);
      setTitle("");
      refreshClips();
    } catch (e) {
      setMsg({ text: errorText(e), bad: true });
    }
  };

  if (ready.length === 0) {
    return (
      <Page title="Clip Picker" subtitle="Watch a full recording and cut your own clips.">
        <Empty>No recordings yet. They appear here once a stream is imported.</Empty>
      </Page>
    );
  }

  return (
    <Page
      title="Clip Picker"
      subtitle="Watch the full recording, mark a start and end, and make your own clip. Keys: Space play/pause · J/L back/forward 5 s · I mark in · O mark out."
      actions={<Button onClick={() => go("clips")}>Open AI Clip Studio</Button>}
    >
      {msg && <div className="mb-4"><Notice tone={msg.bad ? "bad" : "good"}>{msg.text}</Notice></div>}
      <div className="mb-4">
        <select className={`${inputCls} w-96`} value={stream?.id ?? ""} onChange={(e) => setSid(Number(e.target.value))}>
          {ready.map((s) => <option key={s.id} value={s.id}>{s.title} · {duration(s.duration_s)}</option>)}
        </select>
      </div>
      {stream && (
        <div className="grid grid-cols-[minmax(0,1fr)_360px] gap-5">
          <div className="min-w-0 space-y-4">
            <video ref={video} key={stream.id} className="aspect-video w-full rounded-lg bg-black" src={fileUrl(stream.file_path!)} controls preload="metadata"
              onTimeUpdate={(e) => setNow(e.currentTarget.currentTime)} />
            {/* Timeline: AI and manual clips, your marks, and the playhead. Click to jump. */}
            <div className="relative h-8 cursor-pointer rounded bg-ink-800" title="Click to jump"
              onClick={(e) => { const r = e.currentTarget.getBoundingClientRect(); seek(((e.clientX - r.left) / r.width) * total); }}>
              {streamClips.map((c) => (
                <div key={c.id} className="absolute top-1 h-6 rounded-sm bg-sky-500/50" title={c.title ?? ""}
                  style={{ left: `${(c.start_s / total) * 100}%`, width: `${Math.max(((c.end_s - c.start_s) / total) * 100, 0.3)}%` }} />
              ))}
              {inAt != null && outAt != null && outAt > inAt && (
                <div className="absolute top-0 h-8 rounded-sm bg-amber-400/40 ring-1 ring-amber-300" style={{ left: `${(inAt / total) * 100}%`, width: `${((outAt - inAt) / total) * 100}%` }} />
              )}
              <div className="absolute top-0 h-8 w-0.5 bg-white" style={{ left: `${(now / total) * 100}%` }} />
            </div>
            <div className="text-xs text-ink-500">Blue: clips already made. Yellow: your selection. White line: where you are.</div>
            <Card>
              <div className="flex flex-wrap items-end gap-3">
                <Button onClick={() => setIn(video.current?.currentTime ?? now)}>Mark in (I)</Button>
                <Button onClick={() => setOut(video.current?.currentTime ?? now)}>Mark out (O)</Button>
                <div className="text-sm">
                  <span className="text-ink-500">In</span> <button className="tabular-nums text-sky-300 hover:underline" onClick={() => inAt != null && seek(inAt)}>{inAt != null ? duration(inAt) : "—"}</button>
                  <span className="ml-3 text-ink-500">Out</span> <button className="tabular-nums text-sky-300 hover:underline" onClick={() => outAt != null && seek(outAt)}>{outAt != null ? duration(outAt) : "—"}</button>
                  <span className={`ml-3 ${len == null || ok ? "text-ink-300" : "text-red-300"}`}>{len != null ? `${duration(len)} long${ok ? "" : ` · must be ${MIN}–${MAX} s`}` : ""}</span>
                </div>
                <div className="min-w-60 flex-1">
                  <Field label="Title (optional)"><input className={inputCls} value={title} onChange={(e) => setTitle(e.target.value)} placeholder="What happens in this clip" /></Field>
                </div>
                <Button variant="primary" disabled={!ok} onClick={create}><Scissors size={14} /> Create clip</Button>
              </div>
            </Card>
          </div>
          <Card title="Transcript">
            <div className="relative mb-2">
              <Search size={14} className="absolute left-2.5 top-2.5 text-ink-500" />
              <input className={`${inputCls} pl-8`} value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Search what was said" />
            </div>
            <div ref={list} className="h-[60vh] overflow-y-auto text-sm">
              {lines == null ? <Empty>Loading…</Empty> : shown.length === 0 ? <Empty>{lines.length ? "No matches." : "No transcript yet."}</Empty> : shown.map((l) => {
                const i = lines.indexOf(l);
                return (
                  <div key={i} data-i={i} className={`group flex gap-2 rounded px-1.5 py-1 ${i === currentLine ? "bg-ink-800" : "hover:bg-ink-850"}`}>
                    <button className="w-14 shrink-0 text-left tabular-nums text-sky-300" onClick={() => seek(l.s)}>{duration(l.s)}</button>
                    <button className="flex-1 text-left text-ink-200" onClick={() => seek(l.s)}>{l.text}</button>
                    <button className="hidden shrink-0 text-ink-500 hover:text-white group-hover:block" title="Start a clip here" onClick={() => { setIn(l.s); seek(l.s); }}><Plus size={14} /></button>
                  </div>
                );
              })}
            </div>
          </Card>
        </div>
      )}
    </Page>
  );
}
