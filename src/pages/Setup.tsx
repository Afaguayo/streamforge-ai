import { open } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Check, Copy, ExternalLink, FolderOpen, RefreshCw } from "lucide-react";
import { useCallback, useEffect, useState, type ReactNode } from "react";
import type { PageId } from "../App";
import { PlatformCard } from "../components/PlatformCard";
import { Badge, Button, Notice } from "../components/ui";
import { api, errorText, type Platform, type Settings, type Toolchain } from "../lib/api";

function Step({ n, done, optional, title, children }: { n: number; done: boolean; optional?: boolean; title: string; children: ReactNode }) {
  return (
    <section className="flex gap-5">
      <div className="flex flex-col items-center">
        <div
          className={`flex h-9 w-9 shrink-0 items-center justify-center rounded-full text-sm font-semibold ${
            done ? "bg-emerald-500 text-ink-950" : "border border-ink-700 bg-ink-900 text-ink-100"
          }`}
        >
          {done ? <Check size={18} strokeWidth={3} /> : n}
        </div>
        <div className="mt-2 w-px flex-1 bg-ink-800" />
      </div>
      <div className="min-w-0 flex-1 pb-10">
        <div className="flex items-center gap-2">
          <h2 className="text-lg font-semibold">{title}</h2>
          {optional && <Badge>Optional</Badge>}
          {done && <Badge tone="good">Done</Badge>}
        </div>
        <div className="mt-3 space-y-3 text-sm text-ink-300">{children}</div>
      </div>
    </section>
  );
}

export default function Setup({ go }: { go: (p: PageId) => void }) {
  const [tools, setTools] = useState<Toolchain | null>(null);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [platforms, setPlatforms] = useState<Platform[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [checking, setChecking] = useState(false);

  const load = useCallback(async () => {
    setSettings(await api.settings());
    setPlatforms(await api.platforms());
  }, []);

  const checkTools = async () => {
    setChecking(true);
    setTools(await api.toolchain());
    setChecking(false);
  };

  useEffect(() => {
    load().catch((e) => setErr(errorText(e)));
    checkTools();
  }, [load]);

  if (!settings) return null;

  const kick = platforms.find((p) => p.id === "kick");
  const publishers = platforms.filter((p) => p.id !== "kick");
  const ffmpegOk = !!tools?.ffmpeg;
  const folderOk = !!settings.recording_dir;
  const kickOk = !!kick?.account || (!!kick?.keys_set && !!settings.kick_channel_slug);
  const publishOk = publishers.some((p) => p.account);
  const required = [ffmpegOk, folderOk, kickOk];
  const doneCount = required.filter(Boolean).length;

  const pickFolder = async () => {
    const dir = await open({ directory: true, title: "Choose the folder OBS saves recordings to" });
    if (typeof dir !== "string") return;
    try {
      setSettings(await api.saveSettings({ ...settings, recording_dir: dir }));
      setErr(null);
    } catch (e) {
      setErr(errorText(e));
    }
  };

  const finish = async () => {
    await api.saveSettings({ ...settings, onboarded: true });
    go("dashboard");
  };

  return (
    <div className="mx-auto max-w-4xl px-8 py-8">
      <header className="mb-8">
        <div className="text-xs font-medium uppercase tracking-wider text-accent">First-time setup</div>
        <h1 className="mt-1 text-[26px] font-semibold tracking-tight">Let's get StreamForge ready</h1>
        <p className="mt-2 text-ink-300">
          Three required steps and one optional one, about 10–15 minutes in total. Each step ticks itself off when it's done, and you can come back here
          anytime from <span className="text-ink-100">Setup guide</span> in the sidebar.
        </p>
        <div className="mt-5 flex items-center gap-3">
          <div className="h-2 flex-1 overflow-hidden rounded-full bg-ink-800">
            <div className="h-full rounded-full bg-emerald-500 transition-all" style={{ width: `${(doneCount / required.length) * 100}%` }} />
          </div>
          <span className="text-sm tabular-nums text-ink-300">{doneCount} of {required.length} required steps done</span>
        </div>
      </header>

      {err && (
        <div className="mb-6">
          <Notice tone="bad">{err}</Notice>
        </div>
      )}

      <Step n={1} done={ffmpegOk} title="Install FFmpeg">
        <p>StreamForge uses FFmpeg to read and edit your videos.</p>
        {!tools ? (
          <p>Checking…</p>
        ) : ffmpegOk ? (
          <p>
            Found <span className="text-ink-100">{tools.ffmpeg?.replace("ffmpeg version ", "").split(" ")[0]}</span>
            {tools.nvenc ? " with NVIDIA GPU encoding." : " (CPU encoding)."}
          </p>
        ) : (
          <>
            <p>Not found. Open <span className="text-ink-100">Terminal</span> (or PowerShell), run this command, then close and reopen StreamForge:</p>
            <div className="flex items-center gap-2 rounded-lg border border-ink-700 bg-ink-950 px-3 py-2">
              <code className="flex-1 text-ink-100">winget install Gyan.FFmpeg</code>
              <Button onClick={() => navigator.clipboard.writeText("winget install Gyan.FFmpeg")}><Copy size={13} /> Copy</Button>
            </div>
            <div className="flex gap-2">
              <Button onClick={checkTools} disabled={checking}><RefreshCw size={14} /> {checking ? "Checking…" : "Check again"}</Button>
              <Button onClick={() => openUrl("https://www.gyan.dev/ffmpeg/builds/")}>Manual download <ExternalLink size={13} /></Button>
            </div>
          </>
        )}
      </Step>

      <Step n={2} done={folderOk} title="Choose your recording folder">
        <p>
          StreamForge picks up your stream recordings from the folder OBS saves them to. In OBS this is <span className="text-ink-100">Settings → Output → Recording → Recording Path</span>.
          Set OBS's <span className="text-ink-100">Recording Format</span> to <span className="text-ink-100">MKV</span> so a crash never ruins a recording.
        </p>
        <div className="flex items-center gap-3">
          <Button variant={folderOk ? "ghost" : "primary"} onClick={pickFolder}>
            <FolderOpen size={15} /> {folderOk ? "Change folder" : "Choose folder"}
          </Button>
          {folderOk && <code className="truncate text-ink-100">{settings.recording_dir}</code>}
        </div>
        <p className="text-xs text-ink-500">Make sure OBS is set to record while you stream (Settings → Advanced → Automatically record when streaming).</p>
      </Step>

      <Step n={3} done={kickOk} title="Connect Kick">
        <p>This lets StreamForge notice when your stream starts and ends, so each recording gets the right title. Open the one-time app setup, follow the steps, then sign in.</p>
        {kick && <PlatformCard p={kick} onChanged={load} />}
      </Step>

      <Step n={4} done={publishOk} optional title="Connect where you post">
        <p>Connect YouTube, TikTok and Facebook + Instagram now or later. You'll need them once publishing arrives. Each one has the same one-time app setup.</p>
        <div className="divide-y divide-ink-800 rounded-xl border border-ink-800 bg-ink-900">
          {publishers.map((p) => (
            <div key={p.id} className="flex items-center justify-between px-4 py-3">
              <span className="text-ink-100">{p.name}</span>
              {p.account ? <Badge tone="good">Connected as {p.account.display_name}</Badge> : <Badge>Not connected</Badge>}
            </div>
          ))}
        </div>
        <Button onClick={() => go("connections")}>Open Account Connections</Button>
      </Step>

      <div className="flex items-center justify-between rounded-xl border border-ink-800 bg-ink-900 px-5 py-4">
        <span className="text-sm text-ink-300">
          {doneCount === required.length ? "All set. StreamForge will take it from here after your next stream." : "You can finish the remaining steps later."}
        </span>
        <Button variant="primary" onClick={finish}>{doneCount === required.length ? "Finish setup" : "Continue to dashboard"}</Button>
      </div>
    </div>
  );
}
