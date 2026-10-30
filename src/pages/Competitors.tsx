import { openUrl } from "@tauri-apps/plugin-opener";
import { Plus, RefreshCw, Trash2 } from "lucide-react";
import { useState } from "react";
import { Badge, Button, Card, Empty, Field, Notice, Page, inputCls } from "../components/ui";
import { api, errorText } from "../lib/api";
import { ago, duration } from "../lib/format";
import { usePoll } from "../lib/usePoll";

export default function Competitors() {
  const { data: list, refresh } = usePoll(api.watchlist, 6000);
  const [handle, setHandle] = useState("");
  const [msg, setMsg] = useState<{ text: string; bad?: boolean } | null>(null);
  const act = async (f: () => Promise<unknown>, ok: string) => {
    try {
      await f();
      setMsg({ text: ok });
      refresh();
    } catch (e) {
      setMsg({ text: errorText(e), bad: true });
    }
  };

  return (
    <Page
      title="Competitor Watchlist"
      subtitle="Public YouTube stats for creators you follow, refreshed daily. For inspiration only; nothing is downloaded or reposted."
      actions={<Button onClick={() => act(() => api.refreshWatchlist(), "Refreshing…")}><RefreshCw size={14} /> Refresh</Button>}
    >
      {msg && <div className="mb-4"><Notice tone={msg.bad ? "bad" : "info"}>{msg.text}</Notice></div>}
      <Card>
        <div className="flex items-end gap-2">
          <div className="flex-1">
            <Field label="Add a creator" hint="Their YouTube @handle (or a channel link). TikTok and Instagram have no API for other creators' stats.">
              <input className={inputCls} value={handle} onChange={(e) => setHandle(e.target.value)} placeholder="@creator" />
            </Field>
          </div>
          <Button variant="primary" disabled={!handle.trim()} onClick={() => act(async () => { await api.addWatch(handle); setHandle(""); }, "Added; fetching their channel.")}><Plus size={14} /> Add</Button>
        </div>
      </Card>
      <div className="mt-5 space-y-4">
        {!list?.length ? (
          <Empty>No creators yet.</Empty>
        ) : (
          list.map((w) => {
            const s = w.summary;
            return (
              <Card key={w.id}>
                <div className="flex items-start justify-between gap-4">
                  <div>
                    <div className="text-base font-medium">{w.title ?? w.handle}</div>
                    <div className="mt-1 flex flex-wrap gap-4 text-sm text-ink-300">
                      {w.subscribers != null && <span>{w.subscribers.toLocaleString()} subscribers</span>}
                      {s.posts_per_week != null && <span>{s.posts_per_week} posts/week</span>}
                      {s.avg_views != null && <span>{s.avg_views.toLocaleString()} avg views</span>}
                      {s.pct_shorts != null && <span>{s.pct_shorts}% shorts</span>}
                      <span className="text-ink-500">{w.checked_at ? `checked ${ago(w.checked_at)}` : "not checked yet"}</span>
                    </div>
                    {w.error && <div className="mt-1 text-sm text-red-300">{w.error}</div>}
                  </div>
                  <Button variant="danger" onClick={() => act(() => api.removeWatch(w.id), "Removed.")}><Trash2 size={14} /></Button>
                </div>
                {s.breakout && (
                  <div className="mt-3 rounded-lg border border-amber-900/50 bg-amber-950/20 px-3 py-2 text-sm">
                    <Badge tone="warn">Breakout</Badge> <span className="ml-1">“{s.breakout.title}”, {s.breakout.views.toLocaleString()} views, 3× their average</span>
                  </div>
                )}
                {!!s.recent?.length && (
                  <div className="mt-4 grid grid-cols-5 gap-3">
                    {s.recent.slice(0, 5).map((v) => (
                      <button key={v.id} className="text-left" onClick={() => openUrl(`https://youtu.be/${v.id}`)}>
                        <img src={v.thumb} className="aspect-video w-full rounded object-cover" />
                        <div className="mt-1 line-clamp-2 text-xs">{v.title}</div>
                        <div className="text-[11px] text-ink-500">{v.views.toLocaleString()} · {duration(v.seconds)}</div>
                      </button>
                    ))}
                  </div>
                )}
              </Card>
            );
          })
        )}
      </div>
    </Page>
  );
}
