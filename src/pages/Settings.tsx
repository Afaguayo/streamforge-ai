import { open } from "@tauri-apps/plugin-dialog";
import { openPath } from "@tauri-apps/plugin-opener";
import { FolderOpen } from "lucide-react";
import { useEffect, useState } from "react";
import { Badge, Button, Card, Field, Notice, Page, Toggle, inputCls } from "../components/ui";
import { api, errorText, type Settings, type Toolchain } from "../lib/api";

export default function SettingsPage() {
  const [s, setS] = useState<Settings | null>(null);
  const [tools, setTools] = useState<Toolchain | null>(null);
  const [dataDir, setDataDir] = useState("");
  const [msg, setMsg] = useState<{ tone: "good" | "bad"; text: string } | null>(null);
  const [dirty, setDirty] = useState(false);

  useEffect(() => {
    api.settings().then(setS);
    api.toolchain().then(setTools);
    api.dataDir().then(setDataDir);
  }, []);

  if (!s) return null;
  const set = (patch: Partial<Settings>) => {
    setS({ ...s, ...patch });
    setDirty(true);
  };

  const pick = async (key: "recording_dir" | "output_dir") => {
    const dir = await open({ directory: true, defaultPath: s[key] ?? undefined });
    if (typeof dir === "string") set({ [key]: dir });
  };

  const save = async () => {
    try {
      setS(await api.saveSettings(s));
      setDirty(false);
      setMsg({ tone: "good", text: "Settings saved." });
    } catch (e) {
      setMsg({ tone: "bad", text: errorText(e) });
    }
  };

  return (
    <Page title="Settings" actions={<Button variant="primary" disabled={!dirty} onClick={save}>Save changes</Button>}>
      {msg && (
        <div className="mb-4">
          <Notice tone={msg.tone}>{msg.text}</Notice>
        </div>
      )}
      <div className="space-y-5">
        <Card title="Storage">
          <div className="space-y-5">
            <Field label="Recording folder" hint="Where OBS saves your stream recordings. New files here are imported automatically once they finish writing.">
              <div className="flex gap-2">
                <input className={inputCls} readOnly value={s.recording_dir ?? ""} placeholder="Not set" />
                <Button onClick={() => pick("recording_dir")}><FolderOpen size={15} /> Choose</Button>
              </div>
            </Field>
            <Field label="Output folder" hint="Where clips, thumbnails and exports will go (used from Phase 3). Defaults to the app data folder.">
              <div className="flex gap-2">
                <input className={inputCls} readOnly value={s.output_dir ?? ""} placeholder={dataDir} />
                <Button onClick={() => pick("output_dir")}><FolderOpen size={15} /> Choose</Button>
              </div>
            </Field>
            <div className="flex items-center justify-between text-sm text-ink-300">
              <span>App data and database: <code className="text-ink-100">{dataDir}</code></span>
              <Button onClick={() => openPath(dataDir)}>Open</Button>
            </div>
          </div>
        </Card>

        <Card title="Kick monitoring">
          <Field label="Check every (seconds)" hint="30–900. Lower notices stream start/end faster.">
            <input type="number" min={30} max={900} className={`${inputCls} w-32`} value={s.kick_poll_secs} onChange={(e) => set({ kick_poll_secs: Number(e.target.value) })} />
          </Field>
        </Card>

        <Card title="Performance">
          <div className="space-y-5">
            <Toggle checked={s.use_gpu} onChange={(v) => set({ use_gpu: v })} label="Use NVIDIA GPU encoding (NVENC)" hint="Falls back to CPU when NVENC isn't available." />
            <Toggle checked={s.pause_while_live} onChange={(v) => set({ pause_while_live: v })} label="Pause heavy processing while I'm live" hint="Keeps your stream and game smooth. Light tasks like reading metadata still run." />
            <div className="rounded-lg bg-ink-950 px-4 py-3 text-sm">
              <div className="mb-2 font-medium">Detected on this PC</div>
              {!tools ? (
                <span className="text-ink-500">Checking…</span>
              ) : (
                <div className="space-y-1 text-ink-300">
                  <div>FFmpeg: {tools.ffmpeg ? <span className="text-ink-100">{tools.ffmpeg.replace("ffmpeg version ", "").split(" ")[0]}</span> : <Badge tone="bad">not found on PATH</Badge>}</div>
                  <div>GPU: <span className="text-ink-100">{tools.gpu ?? "no NVIDIA GPU detected"}</span></div>
                  <div>NVENC: {tools.nvenc ? <Badge tone="good">available</Badge> : <Badge tone="warn">unavailable — CPU encoding</Badge>}</div>
                </div>
              )}
            </div>
          </div>
        </Card>

        <Card title="Publishing">
          <div className="text-sm">
            <div className="flex items-center gap-2 font-medium">Review before publishing <Badge tone="good">On</Badge></div>
            <p className="mt-1 text-ink-500">Every post waits for your approval. Full automation becomes available once publishing is built (Phase 6).</p>
          </div>
        </Card>
      </div>
    </Page>
  );
}
