import { open } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { FolderOpen, Plus, RefreshCw } from "lucide-react";
import { useState } from "react";
import type { PageId } from "../App";
import { StreamStatus } from "../components/StreamBits";
import { Badge, Button, Card, Empty, Notice, Page } from "../components/ui";
import { api, errorText } from "../lib/api";
import { ago, bytes, duration } from "../lib/format";
import { usePoll } from "../lib/usePoll";

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
