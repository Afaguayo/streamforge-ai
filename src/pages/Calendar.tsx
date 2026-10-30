import { ChevronLeft, ChevronRight } from "lucide-react";
import { useState } from "react";
import type { PageId } from "../App";
import { Badge, Button, Card, Notice, Page } from "../components/ui";
import { api, errorText, type Post } from "../lib/api";
import { usePoll } from "../lib/usePoll";

const dayKey = (d: Date) => `${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`;
const COLORS = { youtube: "border-red-500/60 bg-red-950/40", tiktok: "border-cyan-400/60 bg-cyan-950/40" } as const;

export default function Calendar({ go }: { go: (p: PageId) => void }) {
  const { data: posts, refresh } = usePoll(api.posts, 5000);
  const [month, setMonth] = useState(() => { const d = new Date(); return new Date(d.getFullYear(), d.getMonth(), 1); });
  const [msg, setMsg] = useState<{ text: string; bad?: boolean } | null>(null);

  const start = new Date(month);
  start.setDate(1 - ((month.getDay() + 6) % 7)); // weeks start Monday
  const days = Array.from({ length: 42 }, (_, i) => new Date(start.getFullYear(), start.getMonth(), start.getDate() + i));
  const when = (p: Post) => p.scheduled_at ?? p.published_at;
  const byDay = new Map<string, Post[]>();
  for (const p of posts ?? []) {
    const w = when(p);
    if (!w || !["scheduled", "published", "uploading"].includes(p.status)) continue;
    const k = dayKey(new Date(w));
    byDay.set(k, [...(byDay.get(k) ?? []), p]);
  }
  const drafts = (posts ?? []).filter((p) => p.status === "draft" || p.status === "failed");

  // Dropping keeps the post's time of day (drafts default to 18:00 local).
  const drop = async (postId: number, day: Date) => {
    const p = posts?.find((x) => x.id === postId);
    if (!p) return;
    const prev = p.scheduled_at ? new Date(p.scheduled_at) : null;
    const at = new Date(day.getFullYear(), day.getMonth(), day.getDate(), prev?.getHours() ?? 18, prev?.getMinutes() ?? 0);
    if (at.getTime() < Date.now()) {
      setMsg({ text: "That time has already passed; pick a later day.", bad: true });
      return;
    }
    try {
      await api.schedulePost(postId, at.toISOString());
      setMsg({ text: `Scheduled for ${at.toLocaleString()}.` });
      refresh();
    } catch (e) {
      setMsg({ text: errorText(e), bad: true });
    }
  };

  const chip = (p: Post) => (
    <div
      key={p.id}
      draggable={p.status !== "published" && p.status !== "uploading"}
      onDragStart={(e) => e.dataTransfer.setData("text/plain", String(p.id))}
      title={p.title}
      className={`cursor-grab truncate rounded border px-1.5 py-0.5 text-[11px] ${COLORS[p.platform]} ${p.status === "published" ? "opacity-60" : ""}`}
    >
      {p.status === "published" ? "✓ " : ""}{p.scheduled_at ? new Date(p.scheduled_at).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }) + " " : ""}{p.title}
    </div>
  );

  return (
    <Page
      title="Publishing Calendar"
      subtitle="Drag a draft onto a day to schedule it, or drag a scheduled post to move it."
      actions={<Button onClick={() => go("uploads")}>Upload Queue</Button>}
    >
      {msg && <div className="mb-4"><Notice tone={msg.bad ? "bad" : "good"}>{msg.text}</Notice></div>}
      <div className="flex gap-5">
        <Card className="flex-1">
          <div className="mb-3 flex items-center justify-between">
            <button className="rounded p-1 hover:bg-ink-800" onClick={() => setMonth(new Date(month.getFullYear(), month.getMonth() - 1, 1))}><ChevronLeft size={18} /></button>
            <div className="font-medium">{month.toLocaleString(undefined, { month: "long", year: "numeric" })}</div>
            <button className="rounded p-1 hover:bg-ink-800" onClick={() => setMonth(new Date(month.getFullYear(), month.getMonth() + 1, 1))}><ChevronRight size={18} /></button>
          </div>
          <div className="grid grid-cols-7 gap-1 text-center text-[11px] uppercase tracking-wider text-ink-500">
            {["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"].map((d) => <div key={d} className="pb-1">{d}</div>)}
          </div>
          <div className="grid grid-cols-7 gap-1">
            {days.map((d) => {
              const inMonth = d.getMonth() === month.getMonth();
              const today = dayKey(d) === dayKey(new Date());
              return (
                <div
                  key={d.toISOString()}
                  onDragOver={(e) => e.preventDefault()}
                  onDrop={(e) => drop(Number(e.dataTransfer.getData("text/plain")), d)}
                  className={`h-28 space-y-1 overflow-hidden rounded-md border p-1.5 ${inMonth ? "border-ink-800 bg-ink-950" : "border-transparent bg-ink-900/40 opacity-50"} ${today ? "ring-1 ring-accent" : ""}`}
                >
                  <div className="text-right text-xs text-ink-500">{d.getDate()}</div>
                  {(byDay.get(dayKey(d)) ?? []).map(chip)}
                </div>
              );
            })}
          </div>
          <div className="mt-3 flex gap-3 text-xs text-ink-500">
            <span className="flex items-center gap-1"><span className="h-2 w-2 rounded-sm bg-red-500" /> YouTube</span>
            <span className="flex items-center gap-1"><span className="h-2 w-2 rounded-sm bg-cyan-400" /> TikTok</span>
          </div>
        </Card>
        <Card title="Drafts" className="w-64 shrink-0" right={<Badge>{drafts.length}</Badge>}>
          {drafts.length === 0 ? <p className="text-sm text-ink-500">No drafts. Approve clips in the Clip Studio.</p> : <div className="space-y-1.5">{drafts.map(chip)}</div>}
        </Card>
      </div>
    </Page>
  );
}
