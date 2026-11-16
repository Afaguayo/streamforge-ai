import { openUrl } from "@tauri-apps/plugin-opener";
import { CalendarClock, ExternalLink, RotateCcw, Send, Trash2, Undo2 } from "lucide-react";
import { useState } from "react";
import type { PageId } from "../App";
import { StageView } from "../components/StreamBits";
import { Badge, Button, Card, Empty, Facts, Field, Notice, Page, Stepper, Tabs, inputCls, useConfirm } from "../components/ui";
import { api, errorText, fileUrl, type Post } from "../lib/api";
import { ago } from "../lib/format";
import { POST_STEPS, platformName, postStage, privacyLabel } from "../lib/stages";
import { usePoll } from "../lib/usePoll";

/** "2026-10-30T18:00" (local, for <input type=datetime-local>) ↔ UTC ISO. */
export const toLocalInput = (iso: string) => {
  const d = new Date(iso);
  return new Date(d.getTime() - d.getTimezoneOffset() * 60000).toISOString().slice(0, 16);
};
export const fromLocalInput = (v: string) => new Date(v).toISOString();

function PostCard({ p, refresh, notify }: { p: Post; refresh: () => void; notify: (m: string, bad?: boolean) => void }) {
  const [editingLive, setEditingLive] = useState(false);
  const confirm = useConfirm();
  const published = p.status === "published";
  // Published YouTube videos can be edited too: the change is sent to YouTube.
  const editable = ["draft", "scheduled", "failed"].includes(p.status) || (published && p.platform === "youtube" && editingLive);
  const [title, setTitle] = useState(p.title);
  const [desc, setDesc] = useState(p.description);
  const [tags, setTags] = useState((JSON.parse(p.tags) as string[]).join(", "));
  const [privacy, setPrivacy] = useState(p.privacy);
  const [when, setWhen] = useState(p.scheduled_at ? toLocalInput(p.scheduled_at) : "");
  const dirty = title !== p.title || desc !== p.description || privacy !== p.privacy || tags !== (JSON.parse(p.tags) as string[]).join(", ");
  const stage = postStage(p);

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
  const remove = async () => {
    if (!(await confirm({ title: "Delete this draft?", body: `“${p.title}” will not be posted to ${platformName(p.platform)}. The clip itself is kept.`, confirm: "Delete draft" }))) return;
    run(() => api.deletePost(p.id), "Draft deleted.");
  };

  return (
    <Card>
      <div className="flex flex-wrap gap-4 sm:flex-nowrap">
        {p.cover ? <img alt="" src={fileUrl(p.cover)} className={`${p.clip_id ? "h-40 w-[90px]" : "h-[90px] w-40"} shrink-0 rounded-md object-cover`} /> : <div className="h-24 w-24 shrink-0 rounded-md bg-ink-800" />}
        <div className="min-w-0 flex-1 space-y-3">
          <div className="flex flex-wrap items-center gap-2 text-sm">
            <Badge tone="info">{platformName(p.platform)}</Badge>
            <Badge>{p.clip_id ? "Clip" : "Full stream"}</Badge>
            <span className="text-ink-300">from “{p.media_title}”</span>
          </div>
          <Stepper steps={POST_STEPS} current={stage.step} failed={stage.failed} finished={stage.finished} />
          <StageView stage={stage} />
          {editable ? (
            <>
              <Field label="Title"><input className={inputCls} value={title} onChange={(e) => setTitle(e.target.value)} /></Field>
              <Field label={p.platform === "tiktok" ? "Caption (hashtags are added from tags)" : "Description"}>
                <textarea className={`${inputCls} h-20 resize-y`} value={desc} onChange={(e) => setDesc(e.target.value)} />
              </Field>
              <div className="grid gap-3 sm:grid-cols-3">
                <Field label="Tags / hashtags" hint="Comma separated"><input className={inputCls} value={tags} onChange={(e) => setTags(e.target.value)} /></Field>
                <Field label="Who can see it" hint={p.platform === "tiktok" ? "Unaudited apps: only private posts" : "Unaudited projects: YouTube keeps uploads private"}>
                  <select className={inputCls} value={privacy} onChange={(e) => setPrivacy(e.target.value as Post["privacy"])}>
                    <option value="private">Private</option>
                    <option value="unlisted">{p.platform === "tiktok" ? "Friends" : "Unlisted"}</option>
                    <option value="public">Public</option>
                  </select>
                </Field>
                {!published && <Field label="Schedule (your time)"><input type="datetime-local" className={inputCls} value={when} onChange={(e) => setWhen(e.target.value)} /></Field>}
              </div>
              {published ? (
                <div className="flex flex-wrap gap-2">
                  <Button variant="primary" disabled={!dirty} onClick={() => run(async () => setEditingLive(false), "Saved. Sending the change to YouTube; it shows on the Progress page.")}><Send size={14} /> Save to YouTube</Button>
                  <Button onClick={() => setEditingLive(false)}>Cancel</Button>
                </div>
              ) : (
                <div className="flex flex-wrap gap-2">
                  <Button variant="primary" onClick={() => run(() => api.schedulePost(p.id, null), p.status === "failed" ? "Trying the upload again. Progress shows here." : "Uploading now. Progress shows here.")}>
                    {p.status === "failed" ? <><RotateCcw size={14} /> Try again</> : <><Send size={14} /> Publish now</>}
                  </Button>
                  <Button disabled={!when} onClick={() => run(() => api.schedulePost(p.id, fromLocalInput(when)), "Scheduled.")}><CalendarClock size={14} /> {p.status === "scheduled" ? "Reschedule" : "Schedule"}</Button>
                  {dirty && <Button onClick={() => run(async () => {}, "Saved.")}>Save changes</Button>}
                  {p.status === "scheduled" && <Button onClick={() => run(() => api.unschedulePost(p.id), "Moved back to drafts.")}><Undo2 size={14} /> Unschedule</Button>}
                  <Button variant="danger" onClick={remove}><Trash2 size={14} /> Delete</Button>
                </div>
              )}
            </>
          ) : (
            <>
              <div className="text-base font-medium">{p.title}</div>
              {p.status === "published" && (
                <div className="space-y-2">
                  <Facts
                    items={[
                      ["Who can see it", privacyLabel(p.platform, p.privacy)],
                      ["Published", p.published_at ? `${new Date(p.published_at).toLocaleString()} (${ago(p.published_at)})` : null],
                      ["Views", p.views != null ? p.views.toLocaleString() : "Stats appear after the next refresh"],
                      ["Likes", p.likes != null ? p.likes.toLocaleString() : null],
                      ["Comments", p.comments != null ? p.comments.toLocaleString() : null],
                    ]}
                  />
                  <div className="flex flex-wrap items-center gap-4 text-sm">
                    {p.url && <button className="inline-flex items-center gap-1 text-sky-300 hover:underline" onClick={() => openUrl(p.url!)}>Open the video <ExternalLink size={12} /></button>}
                    {p.platform === "youtube" && <button className="text-sky-300 hover:underline" onClick={() => setEditingLive(true)}>Edit title, description or privacy</button>}
                  </div>
                </div>
              )}
              {p.status === "uploading" && <div className="text-[13px] text-ink-400">Sending to {platformName(p.platform)} as {privacyLabel(p.platform, p.privacy).toLowerCase()}. You can leave this page; the upload continues.</div>}
              {p.error && <Notice tone="warn">{p.error}</Notice>}
            </>
          )}
        </div>
      </div>
    </Card>
  );
}

const EMPTY: Record<Post["status"], string> = {
  draft: "Approve a clip in AI clips and its drafts appear here for you to check and publish.",
  scheduled: "Pick a time on a draft and press Schedule. It goes out by itself.",
  uploading: "Nothing is uploading right now.",
  published: "Posts you publish are listed here with their link and stats.",
  failed: "No failed uploads. If one fails, the reason and a Try again button show here.",
};

export default function UploadQueue({ go }: { go: (p: PageId) => void }) {
  const { data: posts, refresh } = usePoll(api.posts, 3000);
  const confirm = useConfirm();
  const [tab, setTab] = useState<Post["status"]>("draft");
  const [msg, setMsg] = useState<{ text: string; bad?: boolean } | null>(null);
  const shown = (posts ?? []).filter((p) => p.status === tab);
  const count = (s: Post["status"]) => (posts ?? []).filter((p) => p.status === s).length;

  return (
    <Page title="Upload queue" subtitle="Nothing is posted until you press Publish or schedule it. Approve clips in AI clips to create drafts here.">
      {msg && <div className="mb-4"><Notice tone={msg.bad ? "bad" : "good"}>{msg.text}</Notice></div>}
      {count("failed") > 0 && tab !== "failed" && (
        <div className="mb-4"><Notice tone="bad"><div className="flex items-center justify-between gap-4"><span>{count("failed")} upload{count("failed") === 1 ? "" : "s"} failed.</span><Button size="sm" onClick={() => setTab("failed")}>See why</Button></div></Notice></div>
      )}
      <div className="mb-4">
        <Tabs<Post["status"]>
          value={tab}
          onChange={setTab}
          tabs={[
            { id: "draft", label: "Drafts", count: count("draft") },
            { id: "scheduled", label: "Scheduled", count: count("scheduled") },
            { id: "uploading", label: "Uploading", count: count("uploading") },
            { id: "published", label: "Published", count: count("published") },
            { id: "failed", label: "Failed", count: count("failed") },
          ]}
        />
      </div>
      {tab === "failed" && count("failed") > 0 && (
        <div className="mb-4">
          <Button variant="danger" onClick={async () => {
            if (!(await confirm({ title: `Delete ${count("failed")} failed post${count("failed") === 1 ? "" : "s"}?`, body: "They are removed from the queue and will not be retried. The clips are kept.", confirm: "Delete all failed" }))) return;
            try { const n = await api.deleteFailedPosts(); setMsg({ text: `Deleted ${n} failed post(s).` }); refresh(); } catch (e) { setMsg({ text: errorText(e), bad: true }); }
          }}><Trash2 size={14} /> Delete all failed</Button>
        </div>
      )}
      <div className="space-y-4">
        {shown.length === 0 ? (
          <Empty title="Nothing here" action={tab === "draft" ? <Button onClick={() => go("clips")}>Review clips</Button> : undefined}>{EMPTY[tab]}</Empty>
        ) : shown.map((p) => <PostCard key={`${p.id}-${p.status}`} p={p} refresh={refresh} notify={(text, bad) => setMsg({ text, bad })} />)}
      </div>
      <Card title="Not available yet" className="mt-6">
        <p className="text-sm text-ink-300">Instagram Reels and Facebook Reels need Meta developer registration, which is still pending, so they do not appear here yet.</p>
      </Card>
    </Page>
  );
}
