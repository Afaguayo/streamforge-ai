import { open } from "@tauri-apps/plugin-dialog";
import { openPath, openUrl } from "@tauri-apps/plugin-opener";
import { Download, ExternalLink, FolderOpen, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import { Badge, Button, Card, Field, Notice, Page, Toggle, inputCls } from "../components/ui";
import { api, errorText, type Settings, type Toolchain } from "../lib/api";
import { usePoll } from "../lib/usePoll";

function ClaudeKey({ onMsg }: { onMsg: (m: { tone: "good" | "bad"; text: string }) => void }) {
  const [value, setValue] = useState("");
  const { data: engine, refresh } = usePoll(api.engine, 10000);
  const save = async () => {
    try {
      await api.setSecret("llm.api_key", value);
      setValue("");
      refresh();
      onMsg({ tone: "good", text: "Claude API key saved to Windows Credential Manager." });
    } catch (e) {
      onMsg({ tone: "bad", text: errorText(e) });
    }
  };
  return (
    <Field
      label="Claude API key (optional)"
      hint={engine?.claude ? "✓ Saved. Clips are rated from their transcripts and get titles and hooks. Paste a new key to replace it." : "Without it, clips are ranked by loudness, laughter and excitement words. With it, Claude reads each candidate's transcript to rate it and write titles."}
    >
      <div className="flex gap-2">
        <input type="password" className={inputCls} value={value} onChange={(e) => setValue(e.target.value)} placeholder={engine?.claude ? "••••••••••••" : "sk-ant-…"} />
        <Button variant="primary" disabled={!value.trim()} onClick={save}>Save</Button>
        {engine?.claude && <Button variant="danger" onClick={async () => { await api.clearSecret("llm.api_key"); refresh(); }}>Remove</Button>}
        <Button onClick={() => openUrl("https://console.anthropic.com/settings/keys")}>Get a key <ExternalLink size={13} /></Button>
      </div>
    </Field>
  );
}

function GeminiKey({ onMsg }: { onMsg: (m: { tone: "good" | "bad"; text: string }) => void }) {
  const [value, setValue] = useState("");
  const [saved, setSaved] = useState<boolean | null>(null);
  useEffect(() => { api.secretStatus().then((s) => setSaved(!!s["gemini.api_key"])); }, []);
  const save = async () => {
    try {
      await api.setSecret("gemini.api_key", value.trim());
      setValue("");
      setSaved(true);
      onMsg({ tone: "good", text: "Gemini API key saved to Windows Credential Manager." });
    } catch (e) {
      onMsg({ tone: "bad", text: errorText(e) });
    }
  };
  return (
    <Field
      label="Gemini API key for AI thumbnails (optional)"
      hint={saved ? "✓ Saved. Each stream gets an AI thumbnail made from its best moment, selected automatically. About 3–7 US cents per image; there's no free tier for image models." : "Gemini restyles the frame of your best moment into a thumbnail; the headline is added on top. Sends that frame to Google. Paid: about 3–7 US cents per image."}
    >
      <div className="flex gap-2">
        <input type="password" className={inputCls} value={value} onChange={(e) => setValue(e.target.value)} placeholder={saved ? "••••••••••••" : "AIza…"} />
        <Button variant="primary" disabled={!value.trim()} onClick={save}>Save</Button>
        {saved && <Button variant="danger" onClick={async () => { await api.clearSecret("gemini.api_key"); setSaved(false); }}>Remove</Button>}
        <Button onClick={() => openUrl("https://aistudio.google.com/apikey")}>Get a key <ExternalLink size={13} /></Button>
      </div>
    </Field>
  );
}

export default function SettingsPage() {
  const [s, setS] = useState<Settings | null>(null);
  const [tools, setTools] = useState<Toolchain | null>(null);
  const [dataDir, setDataDir] = useState("");
  const [msg, setMsg] = useState<{ tone: "good" | "bad"; text: string } | null>(null);
  const [dirty, setDirty] = useState(false);
  const { data: engine } = usePoll(api.engine, 3000);
  const { data: disks, refresh: refreshDisks } = usePoll(api.disks, 30000);

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
  const platform = (p: string, on: boolean) => set({ publish_platforms: on ? [...new Set([...s.publish_platforms, p])] : s.publish_platforms.filter((x) => x !== p) });
  const ready = engine?.status.whisper_installed && engine.status.model_installed;

  return (
    <Page title="Settings" actions={<Button variant="primary" disabled={!dirty} onClick={save}>Save changes</Button>}>
      {msg && <div className="sticky top-0 z-10 mb-4"><Notice tone={msg.tone}>{msg.text}</Notice></div>}
      <div className="space-y-5">
        <Card title="AI processing">
          <div className="space-y-5">
            <div className="flex items-center justify-between gap-4 rounded-lg bg-ink-950 px-4 py-3">
              <div className="text-sm">
                <div className="font-medium">Transcription engine (whisper.cpp)</div>
                <div className="mt-0.5 text-ink-500">
                  {ready ? `Installed with the ${engine!.status.model} model.` : engine?.installing ? "Installing… progress shows on the Dashboard." : "Not installed. About 1.2 GB with GPU support, plus the model. Runs fully on your PC."}
                </div>
              </div>
              {ready ? <Badge tone="good">Ready</Badge> : <Button variant="primary" disabled={engine?.installing} onClick={async () => { await api.installEngine(); setMsg({ tone: "good", text: "Installing the transcription engine…" }); }}><Download size={14} /> Install</Button>}
            </div>
            <Field label="Speech model" hint="Small understands Spanish and English much better; Base is faster on weak PCs. Changing it downloads the new model next time.">
              <select className={`${inputCls} w-72`} value={s.whisper_model} onChange={(e) => set({ whisper_model: e.target.value as Settings["whisper_model"] })}>
                <option value="small">Small (466 MB, recommended)</option>
                <option value="base">Base (142 MB, faster)</option>
              </select>
            </Field>
            <ClaudeKey onMsg={setMsg} />
            <GeminiKey onMsg={setMsg} />
            <div className="flex items-center justify-between gap-4 rounded-lg bg-ink-950 px-4 py-3">
              <div className="text-sm">
                <div className="font-medium">Speaker colors</div>
                <div className="mt-0.5 text-ink-500">
                  {engine?.status.speakers_installed ? "Installed. Each voice in a clip gets its own color in the captions and transcript." : engine?.installing_speakers ? "Installing… progress shows on the Dashboard." : "Optional, about 100 MB, runs on your PC. Downloads sherpa-onnx from GitHub. Game audio and overlapping voices can confuse it."}
                </div>
              </div>
              {engine?.status.speakers_installed ? <Badge tone="good">Ready</Badge> : <Button disabled={engine?.installing_speakers} onClick={async () => { await api.installSpeakers(); setMsg({ tone: "good", text: "Installing speaker detection…" }); }}><Download size={14} /> Install</Button>}
            </div>
            <Field label="Render the best clips automatically" hint="So they're ready to review when you wake up. 0 = render only when you approve.">
              <input type="number" min={0} max={20} className={`${inputCls} w-32`} value={s.auto_render_top} onChange={(e) => set({ auto_render_top: Number(e.target.value) })} />
            </Field>
            <Field label="Clip layout">
              <select className={`${inputCls} w-72`} value={s.clip_layout} onChange={(e) => set({ clip_layout: e.target.value as Settings["clip_layout"] })}>
                <option value="blur">Gameplay centred on a blurred background</option>
                <option value="facecam">Webcam on top, gameplay below</option>
              </select>
            </Field>
            {s.clip_layout === "facecam" && (
              <Field label="Where your webcam sits in the recording" hint="Percent of the frame: left, top, width, height. The default is the top-right corner.">
                <div className="flex gap-2">
                  {(["Left", "Top", "Width", "Height"] as const).map((label, i) => (
                    <label key={label} className="text-xs text-ink-500">
                      {label}
                      <input type="number" min={0} max={100} className={`${inputCls} mt-1 w-24`} value={Math.round(s.facecam_rect[i] * 100)}
                        onChange={(e) => { const r = [...s.facecam_rect] as Settings["facecam_rect"]; r[i] = Math.min(1, Math.max(0, Number(e.target.value) / 100)); set({ facecam_rect: r }); }} />
                    </label>
                  ))}
                </div>
              </Field>
            )}
            <Toggle checked={s.subtitles} onChange={(v) => set({ subtitles: v })} label="Animated word-by-word captions" hint="Large, high-contrast and placed above where TikTok/Shorts put their buttons." />
          </div>
        </Card>

        <Card title="Censor">
          <div className="space-y-5">
            <Toggle checked={s.censor_enabled} onChange={(v) => set({ censor_enabled: v })} label="Censor the N-word" hint="Masks it in transcripts and captions and bleeps it in clips and full stream uploads. It relies on speech recognition, which can miss a word, so check anything going public. You can bleep extra words by hand in a clip's transcript." />
            {s.censor_enabled && (
              <>
                <Field label="Sound">
                  <select className={`${inputCls} w-72`} value={s.censor_style} onChange={(e) => set({ censor_style: e.target.value as Settings["censor_style"] })}>
                    <option value="beep">Beep</option>
                    <option value="mute">Silence</option>
                  </select>
                </Field>
                <Field label="Other words to censor" hint="Whole words, separated by commas. Applies to streams transcribed after saving.">
                  <input className={inputCls} value={s.censor_words.join(", ")} onChange={(e) => set({ censor_words: e.target.value.split(",").map((w) => w.trimStart()) })} />
                </Field>
              </>
            )}
          </div>
        </Card>

        <Card title="Full streams">
          <div className="space-y-5">
            <Toggle checked={s.auto_upload_vod} onChange={(v) => set({ auto_upload_vod: v })} label="Upload each Kick stream to YouTube automatically" hint="After a stream ends: uses your OBS recording if there is one, otherwise downloads the VOD from Kick, then censors it, makes the thumbnail and title, and uploads. Past VODs you add by hand aren't uploaded unless you ask." />
            <Field label="Privacy for those uploads" hint="YouTube keeps uploads private until Google audits the app, whatever you pick here.">
              <select className={`${inputCls} w-72`} value={s.vod_privacy} onChange={(e) => set({ vod_privacy: e.target.value as Settings["vod_privacy"] })}>
                <option value="private">Private</option>
                <option value="unlisted">Unlisted</option>
                <option value="public">Public</option>
              </select>
            </Field>
            <Toggle checked={s.cut_dead_air} onChange={(v) => set({ cut_dead_air: v })} label="Cut dead air from full streams" hint="Removes stretches over 45 seconds where nobody talks and little happens (AFK, loading, queues). Your original recording isn't changed." />
            <Field label="Hashtags for full streams" hint="Added at the end of each description, after the game's own hashtag. YouTube shows the first three above the title.">
              <input className={inputCls} value={s.hashtags.join(" ")} onChange={(e) => set({ hashtags: e.target.value.split(/[\s,]+/) })} />
            </Field>
            <Field label="Links in YouTube descriptions" hint="A handle or a full link. Added to full streams and Shorts. Empty Kick and TikTok use your connected accounts.">
              <div className="grid grid-cols-2 gap-3">
                {([["kick", "Kick"], ["tiktok", "TikTok"], ["instagram", "Instagram"], ["x", "X"], ["discord", "Discord invite"]] as const).map(([k, label]) => (
                  <label key={k} className="text-xs text-ink-500">
                    {label}
                    <input className={`${inputCls} mt-1`} value={s.socials[k]} onChange={(e) => set({ socials: { ...s.socials, [k]: e.target.value } })} />
                  </label>
                ))}
              </div>
            </Field>
          </div>
        </Card>

        <Card title="Publishing">
          <div className="space-y-5">
            <Field label="Create drafts for" hint="When you approve a clip.">
              <div className="flex gap-5 text-sm">
                {[["youtube", "YouTube Shorts"], ["tiktok", "TikTok"]].map(([id, label]) => (
                  <label key={id} className="flex items-center gap-2"><input type="checkbox" checked={s.publish_platforms.includes(id)} onChange={(e) => platform(id, e.target.checked)} /> {label}</label>
                ))}
                <span className="text-ink-500">Instagram/Facebook: pending Meta setup</span>
              </div>
            </Field>
            <Field label="Default privacy">
              <select className={`${inputCls} w-72`} value={s.default_privacy} onChange={(e) => set({ default_privacy: e.target.value as Settings["default_privacy"] })}>
                <option value="private">Private</option>
                <option value="unlisted">Unlisted</option>
                <option value="public">Public</option>
              </select>
            </Field>
            <Toggle checked={s.auto_publish} onChange={(v) => set({ auto_publish: v })} label="Full automation" hint="Off: every post waits for you in the Upload Queue. On: the best clips are approved and scheduled automatically after each stream." />
            {s.auto_publish && (
              <Field label="Posts per day per platform" hint="Automatic posts are spaced evenly through the day.">
                <input type="number" min={1} max={24} className={`${inputCls} w-32`} value={s.posts_per_day} onChange={(e) => set({ posts_per_day: Number(e.target.value) })} />
              </Field>
            )}
          </div>
        </Card>

        <Card title="Storage">
          <div className="space-y-5">
            <Field label="Recording folder" hint="Where OBS saves your stream recordings. New files here are imported automatically once they finish writing.">
              <div className="flex gap-2">
                <input className={inputCls} readOnly value={s.recording_dir ?? ""} placeholder="Not set" />
                <Button onClick={() => pick("recording_dir")}><FolderOpen size={15} /> Choose</Button>
              </div>
            </Field>
            <Field label="Output folder" hint="Where rendered clips, covers and thumbnails go. Defaults to the app data folder.">
              <div className="flex gap-2">
                <input className={inputCls} readOnly value={s.output_dir ?? ""} placeholder={dataDir} />
                <Button onClick={() => pick("output_dir")}><FolderOpen size={15} /> Choose</Button>
              </div>
            </Field>
            <div className="space-y-1.5 text-sm">
              {disks?.map((d) => (
                <div key={d.path} className="flex items-center gap-3">
                  <span className="w-10 text-ink-300">{d.path}</span>
                  <div className="h-2 w-64 overflow-hidden rounded bg-ink-800"><div className={`h-full ${d.free_gb < 10 ? "bg-red-400" : "bg-emerald-500"}`} style={{ width: `${100 - (d.free_gb / d.total_gb) * 100}%` }} /></div>
                  <span className={d.free_gb < 10 ? "text-red-300" : "text-ink-300"}>{d.free_gb.toFixed(1)} GB free of {d.total_gb.toFixed(0)} GB</span>
                </div>
              ))}
              <p className="text-xs text-ink-500">Processing pauses by itself below 2 GB free.</p>
            </div>
            <div className="flex items-center justify-between text-sm text-ink-300">
              <span>App data and database: <code className="text-ink-100">{dataDir}</code></span>
              <div className="flex gap-2">
                <Button onClick={async () => { try { setMsg({ tone: "good", text: await api.cleanTemp() }); refreshDisks(); } catch (e) { setMsg({ tone: "bad", text: errorText(e) }); } }}><Trash2 size={14} /> Delete temp files</Button>
                <Button onClick={() => openPath(dataDir)}>Open</Button>
              </div>
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
            <Toggle checked={s.use_gpu} onChange={(v) => set({ use_gpu: v })} label="Use NVIDIA GPU (NVENC encoding, CUDA transcription)" hint="Falls back to CPU when unavailable. Rendering runs at below-normal priority." />
            <Toggle checked={s.pause_while_live} onChange={(v) => set({ pause_while_live: v })} label="Pause heavy work while I'm live" hint="Transcribing, rendering and uploading wait until your stream ends." />
            <div className="rounded-lg bg-ink-950 px-4 py-3 text-sm">
              <div className="mb-2 font-medium">Detected on this PC</div>
              {!tools ? <span className="text-ink-500">Checking…</span> : (
                <div className="space-y-1 text-ink-300">
                  <div>FFmpeg: {tools.ffmpeg ? <span className="text-ink-100">{tools.ffmpeg.replace("ffmpeg version ", "").split(" ")[0]}</span> : <Badge tone="bad">not found on PATH</Badge>}</div>
                  <div>GPU: <span className="text-ink-100">{tools.gpu ?? "no NVIDIA GPU detected"}</span></div>
                  <div>NVENC: {tools.nvenc ? <Badge tone="good">available</Badge> : <Badge tone="warn">unavailable, CPU encoding</Badge>}</div>
                </div>
              )}
            </div>
          </div>
        </Card>
      </div>
    </Page>
  );
}
