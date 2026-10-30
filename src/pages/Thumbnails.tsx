import { CheckCircle2, RefreshCw } from "lucide-react";
import { useState } from "react";
import { Badge, Button, Card, Empty, Notice, Page } from "../components/ui";
import { api, errorText, fileUrl } from "../lib/api";
import { usePoll } from "../lib/usePoll";

export default function Thumbnails() {
  const { data: thumbs, refresh } = usePoll(api.thumbnails, 5000);
  const { data: clips } = usePoll(api.clips, 8000);
  const [msg, setMsg] = useState<{ text: string; bad?: boolean } | null>(null);
  const streams = [...new Set((thumbs ?? []).map((t) => t.stream_id))];
  const act = async (f: () => Promise<unknown>, ok: string) => {
    try {
      await f();
      setMsg({ text: ok });
      refresh();
    } catch (e) {
      setMsg({ text: errorText(e), bad: true });
    }
  };
  const covers = (clips ?? []).filter((c) => c.cover_path && c.status !== "rejected").slice(0, 12);

  return (
    <Page
      title="Thumbnail Studio"
      subtitle="Three 1280×720 concepts per stream, built from its strongest moments. The selected one is used when the full stream goes to YouTube."
    >
      {msg && <div className="mb-4"><Notice tone={msg.bad ? "bad" : "good"}>{msg.text}</Notice></div>}
      <Notice>
        Concepts are frames from your stream with color grading and bold headlines, ranked by how strong the moment was. AI image generation and background removal aren't built in;
        you can always upload your own thumbnail in YouTube Studio.
      </Notice>
      <div className="mt-5 space-y-5">
        {streams.length === 0 ? (
          <Empty>Thumbnails are made automatically after a stream is analysed.</Empty>
        ) : (
          streams.map((sid) => {
            const set = (thumbs ?? []).filter((t) => t.stream_id === sid);
            return (
              <Card key={sid} title={set[0].stream_title} right={<Button onClick={() => act(() => api.regenerateThumbnails(sid), "Regenerating…")}><RefreshCw size={14} /> Regenerate</Button>}>
                <div className="grid grid-cols-3 gap-4">
                  {set.map((t) => (
                    <button key={t.id} onClick={() => act(() => api.selectThumbnail(t.id), "Thumbnail selected.")} className={`overflow-hidden rounded-lg border-2 text-left ${t.selected ? "border-emerald-500" : "border-transparent hover:border-ink-700"}`}>
                      <img src={`${fileUrl(t.path)}?v=${t.id}`} className="aspect-video w-full object-cover" />
                      <div className="flex items-center justify-between px-2 py-1.5 text-xs">
                        <span className="text-ink-300">#{t.rank} · {t.concept}</span>
                        {t.selected && <Badge tone="good"><CheckCircle2 size={11} /> Selected</Badge>}
                      </div>
                    </button>
                  ))}
                </div>
              </Card>
            );
          })
        )}
        {covers.length > 0 && (
          <Card title="Vertical covers (Shorts, TikTok, Reels)">
            <div className="grid grid-cols-6 gap-3">
              {covers.map((c) => (
                <div key={c.id}>
                  <img src={fileUrl(c.cover_path!)} className="aspect-[9/16] w-full rounded-md object-cover" />
                  <div className="mt-1 truncate text-xs text-ink-300">{c.title}</div>
                </div>
              ))}
            </div>
          </Card>
        )}
      </div>
    </Page>
  );
}
