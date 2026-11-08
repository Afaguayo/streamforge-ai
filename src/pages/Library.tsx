import { open } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { Download, FolderOpen, Plus, RefreshCw, Sparkles, Youtube } from "lucide-react";
import { useState } from "react";
import type { PageId } from "../App";
import { StreamStatus } from "../components/StreamBits";
import { Badge, Button, Card, Empty, Notice, Page, inputCls } from "../components/ui";
import { api, errorText, type KickVod } from "../lib/api";
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
      {vods?.length === 0 && <div className="mt-3"><Empty>Kick has no past broadcasts for this channel.</Empty></div>}
      {vods && vods.length > 0 && (
        <div className="mt-3 divide-y divide-ink-800">
          {vods.map((v) => (
            <div key={v.id} className="flex items-center gap-4 py-2.5 text-sm">
              {v.thumbnail && <img src={v.thumbnail} className="h-12 w-20 shrink-0 rounded object-cover" />}
              <div className="min-w-0 flex-1">
                <div className="truncate font-medium" title={v.title}>{v.title}</div>
                <div className="text-xs text-ink-500">{new Date(v.started_at).toLocaleString()} · {duration(v.duration_s)}</div>
              </div>
              {v.stream_id ? <Badge tone="good">In library</Badge> : <Button onClick={() => add(() => api.importKickVod(v))}><Download size={14} /> Download</Button>}
            </div>
          ))}
        </div>
      )}
    </Card>
  );
}

export default function Library({ go }: { go: (p: PageId) => void }) {
  const { data: streams, refresh } = usePoll(api.streams, 3000);
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
      title="Video Library"
      subtitle="Every recording StreamForge knows about. New files in your recording folder are added automatically."
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
      <Card>
        {!streams?.length ? (
          <Empty>No recordings yet. Import a file or set your OBS recording folder in Settings.</Empty>
        ) : (
          <table className="w-full text-sm">
            <thead className="text-left text-xs uppercase tracking-wider text-ink-500">
              <tr>
                <th className="pb-3 font-medium">Title</th>
                <th className="pb-3 font-medium">Length</th>
                <th className="pb-3 font-medium">Video</th>
                <th className="pb-3 font-medium">Size</th>
                <th className="pb-3 font-medium">Status</th>
                <th className="pb-3 font-medium">Added</th>
                <th />
              </tr>
            </thead>
            <tbody className="divide-y divide-ink-800">
              {streams.map((s) => (
                <tr key={s.id} className="align-middle">
                  <td className="max-w-72 py-3 pr-4">
                    <div className="flex items-center gap-2">
                      <span className="truncate font-medium" title={s.file_path ?? ""}>{s.title}</span>
                      {s.source === "kick" && <Badge tone="live">Kick</Badge>}
                    </div>
                    {s.error && <div className="mt-0.5 truncate text-xs text-red-300" title={s.error}>{s.error}</div>}
                  </td>
                  <td className="py-3 pr-4 tabular-nums">{duration(s.duration_s)}</td>
                  <td className="py-3 pr-4 text-ink-300">{s.width ? `${s.width}×${s.height} · ${s.video_codec} · ${s.fps}fps` : "—"}</td>
                  <td className="py-3 pr-4 tabular-nums text-ink-300">{bytes(s.file_size)}</td>
                  <td className="py-3 pr-4"><StreamStatus s={s.status} /></td>
                  <td className="py-3 pr-4 text-ink-300">{ago(s.created_at)}</td>
                  <td className="py-3">
                    <div className="flex justify-end gap-3 text-ink-300">
                      {s.file_path && (
                        <button title="Show in Explorer" className="hover:text-white" onClick={() => run(() => revealItemInDir(s.file_path!))}>
                          <FolderOpen size={15} />
                        </button>
                      )}
                      {s.file_path && (
                        <button title="Read metadata again" className="hover:text-white" onClick={() => run(() => api.reprobe(s.id))}>
                          <RefreshCw size={15} />
                        </button>
                      )}
                      {s.duration_s != null && (
                        <button title="Find clips (transcribe and analyse)" className="hover:text-white" onClick={() => run(async () => { await api.processStream(s.id); setMsg({ tone: "good", text: "Finding clips; progress is on the Dashboard." }); })}>
                          <Sparkles size={15} />
                        </button>
                      )}
                      {s.duration_s != null && (
                        <button title="Draft the full stream for YouTube" className="hover:text-white" onClick={() => run(async () => { await api.draftFullVideo(s.id); setMsg({ tone: "good", text: "Full-stream draft added to the Upload Queue." }); })}>
                          <Youtube size={15} />
                        </button>
                      )}
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </Card>
    </Page>
  );
}
