import { openUrl } from "@tauri-apps/plugin-opener";
import { CalendarClock, ExternalLink, Send, Trash2, Undo2 } from "lucide-react";
import { useState } from "react";
import { Badge, Button, Card, Empty, Field, Notice, Page, inputCls } from "../components/ui";
import { api, errorText, fileUrl, type Post } from "../lib/api";
import { ago } from "../lib/format";
import { usePoll } from "../lib/usePoll";

const PLATFORM = { youtube: "YouTube", tiktok: "TikTok" } as const;

/** "2026-10-30T18:00" (local, for <input type=datetime-local>) ↔ UTC ISO. */
export const toLocalInput = (iso: string) => {
  const d = new Date(iso);
  return new Date(d.getTime() - d.getTimezoneOffset() * 60000).toISOString().slice(0, 16);
};
export const fromLocalInput = (v: string) => new Date(v).toISOString();

function PostCard({ p, refresh, notify }: { p: Post; refresh: () => void; notify: (m: string, bad?: boolean) => void }) {
  const editable = ["draft", "scheduled", "failed"].includes(p.status);
  const [title, setTitle] = useState(p.title);
  const [desc, setDesc] = useState(p.description);
  const [tags, setTags] = useState((JSON.parse(p.tags) as string[]).join(", "));
  const [privacy, setPrivacy] = useState(p.privacy);
  const [when, setWhen] = useState(p.scheduled_at ? toLocalInput(p.scheduled_at) : "");
  const dirty = title !== p.title || desc !== p.description || privacy !== p.privacy || tags !== (JSON.parse(p.tags) as string[]).join(", ");

  const run = async (f: () => Promise<unknown>, ok: string) => {
    try {
      if (dirty && editable) await api.updatePost(p.id, title, desc, tags.split(",").map((t) => t.trim()).filter(Boolean), privacy);
      await f();
      notify(ok);
      refresh();
    } catch (e) {
      notify(errorText(e), true);
    }
  };

  return (
    <Card>
      <div className="flex gap-4">
        {p.cover ? <img src={fileUrl(p.cover)} className={`${p.clip_id ? "h-40 w-[90px]" : "h-[90px] w-40"} shrink-0 rounded-md object-cover`} /> : <div className="h-24 w-24 shrink-0 rounded-md bg-ink-800" />}
        <div className="min-w-0 flex-1 space-y-3">
          <div className="flex flex-wrap items-center gap-2 text-sm">
            <Badge tone="info">{PLATFORM[p.platform]}</Badge>
            <Badge>{p.clip_id ? "clip" : "full stream"}</Badge>
            <span className="text-ink-500">from “{p.media_title}”</span>
            <span className="ml-auto text-xs text-ink-500">
              {p.status === "scheduled" && p.scheduled_at && `goes out ${new Date(p.scheduled_at).toLocaleString()}`}
              {p.status === "published" && p.published_at && `published ${ago(p.published_at)}`}
            </span>
          </div>
          {editable ? (
            <>
              <Field label="Title"><input className={inputCls} value={title} onChange={(e) => setTitle(e.target.value)} /></Field>
              <Field label={p.platform === "tiktok" ? "Caption (hashtags are added from tags)" : "Description"}>
                <textarea className={`${inputCls} h-20 resize-y`} value={desc} onChange={(e) => setDesc(e.target.value)} />
              </Field>
              <div className="grid grid-cols-3 gap-3">
                <Field label="Tags / hashtags" hint="Comma separated"><input className={inputCls} value={tags} onChange={(e) => setTags(e.target.value)} /></Field>
                <Field label="Privacy" hint={p.platform === "tiktok" ? "Unaudited apps: only private posts" : "Unaudited projects: YouTube keeps uploads private"}>
                  <select className={inputCls} value={privacy} onChange={(e) => setPrivacy(e.target.value as Post["privacy"])}>
                    <option value="private">Private</option>
                    <option value="unlisted">{p.platform === "tiktok" ? "Friends" : "Unlisted"}</option>
                    <option value="public">Public</option>
                  </select>
                </Field>
                <Field label="Schedule (your time)"><input type="datetime-local" className={inputCls} value={when} onChange={(e) => setWhen(e.target.value)} /></Field>
              </div>
              {p.error && <Notice tone={p.status === "failed" ? "bad" : "warn"}>{p.error}</Notice>}
              <div className="flex flex-wrap gap-2">
                <Button variant="primary" onClick={() => run(() => api.schedulePost(p.id, null), "Uploading now. Progress shows here.")}><Send size={14} /> Publish now</Button>
                <Button disabled={!when} onClick={() => run(() => api.schedulePost(p.id, fromLocalInput(when)), "Scheduled.")}><CalendarClock size={14} /> {p.status === "scheduled" ? "Reschedule" : "Schedule"}</Button>
                {dirty && <Button onClick={() => run(async () => {}, "Saved.")}>Save changes</Button>}
                {p.status === "scheduled" && <Button onClick={() => run(() => api.unschedulePost(p.id), "Moved back to drafts.")}><Undo2 size={14} /> Unschedule</Button>}
                <Button variant="danger" onClick={() => run(() => api.deletePost(p.id), "Draft deleted.")}><Trash2 size={14} /> Delete</Button>
              </div>
            </>
          ) : (
            <>
              <div className="text-base font-medium">{p.title}</div>
              {p.status === "uploading" && (
                <div className="h-1.5 overflow-hidden rounded bg-ink-800"><div className="h-full bg-sky-400 transition-all" style={{ width: `${Math.round(p.progress * 100)}%` }} /></div>
              )}
              {p.status === "published" && (
                <div className="flex items-center gap-5 text-sm text-ink-300">
                  {p.views != null ? <span>{p.views.toLocaleString()} views · {p.likes ?? 0} likes · {p.comments ?? 0} comments</span> : <span>Stats appear after the next refresh</span>}
                  {p.url && <button className="inline-flex items-center gap-1 text-sky-300 hover:underline" onClick={() => openUrl(p.url!)}>Open <ExternalLink size={12} /></button>}
                </div>
              )}
              {p.error && <Notice tone="warn">{p.error}</Notice>}
            </>
          )}
        </div>
      </div>
    </Card>
  );
}

export default function UploadQueue() {
  const { data: posts, refresh } = usePoll(api.posts, 3000);
  const [tab, setTab] = useState<Post["status"]>("draft");
  const [msg, setMsg] = useState<{ text: string; bad?: boolean } | null>(null);
  const shown = (posts ?? []).filter((p) => p.status === tab);
  const count = (s: Post["status"]) => (posts ?? []).filter((p) => p.status === s).length;

  return (
    <Page title="Upload Queue" subtitle="Nothing is posted until you press Publish or schedule it. Approve clips in the Clip Studio to create drafts here.">
      {msg && <div className="mb-4"><Notice tone={msg.bad ? "bad" : "good"}>{msg.text}</Notice></div>}
      <div className="mb-4 flex w-fit rounded-lg border border-ink-700 p-0.5 text-sm">
        {(["draft", "scheduled", "uploading", "published", "failed"] as const).map((t) => (
          <button key={t} onClick={() => setTab(t)} className={`rounded-md px-3 py-1.5 capitalize ${tab === t ? "bg-ink-800 text-ink-100" : "text-ink-300"}`}>
            {t} ({count(t)})
          </button>
        ))}
      </div>
      <div className="space-y-4">
        {shown.length === 0 ? <Empty>Nothing here.</Empty> : shown.map((p) => <PostCard key={`${p.id}-${p.status}`} p={p} refresh={refresh} notify={(text, bad) => setMsg({ text, bad })} />)}
      </div>
      <Card title="Not available yet" className="mt-6">
        <p className="text-sm text-ink-300">Instagram Reels and Facebook Reels need Meta's developer registration, which is still pending, so they don't appear here yet.</p>
      </Card>
    </Page>
  );
}
