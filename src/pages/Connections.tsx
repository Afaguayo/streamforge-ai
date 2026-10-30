import { openUrl } from "@tauri-apps/plugin-opener";
import { CheckCircle2, ChevronDown, Copy, ExternalLink, KeyRound, LogIn, LogOut, RefreshCw } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";
import { Badge, Button, Card, Field, Notice, Page, inputCls } from "../components/ui";
import { api, errorText, type Platform, type Settings } from "../lib/api";
import { ago } from "../lib/format";

type Msg = { tone: "good" | "bad" | "info"; text: string } | null;

// One-time developer-app setup per platform, plus the limits that apply to each.
const GUIDE: Record<Platform["id"], { idLabel: string; secretLabel: string; link: string; steps: ReactNode[]; limits: string[] }> = {
  kick: {
    idLabel: "Client ID",
    secretLabel: "Client Secret",
    link: "https://kick.com/settings/developer",
    steps: [
      "On kick.com open Settings → Developer → Create app (two-factor authentication must be on).",
      "Paste the redirect URL below into the app's Redirect URL.",
      "Tick the scopes user:read and channel:read, save, then copy the Client ID and Client Secret here.",
    ],
    limits: ["Kick's public API has no VOD downloads, so recordings come from your OBS recording folder."],
  },
  youtube: {
    idLabel: "Client ID",
    secretLabel: "Client Secret",
    link: "https://console.cloud.google.com/apis/library/youtube.googleapis.com",
    steps: [
      "In Google Cloud Console create a project and enable YouTube Data API v3 (and YouTube Analytics API).",
      "Under Google Auth Platform → Audience pick External, add your Gmail as a test user, or press Publish app.",
      "Under Clients create an OAuth client of type Desktop app and copy its Client ID and Client Secret here. Desktop clients need no redirect URL.",
    ],
    limits: [
      "While the Google app is in Testing, Google ends the sign-in after 7 days. Publishing it removes that (you'll see an \"unverified app\" screen, which is fine for your own channel).",
      "Uploads stay private until Google audits the API project; about 6 uploads a day on the default quota.",
    ],
  },
  tiktok: {
    idLabel: "Client key",
    secretLabel: "Client secret",
    link: "https://developers.tiktok.com/apps",
    steps: [
      "On developers.tiktok.com create an app and add the Login Kit and Content Posting API products.",
      "Choose Desktop as the platform and add the redirect URL below.",
      "Copy the Client key and Client secret here.",
    ],
    limits: ["Until TikTok audits your app, posts can only go to a private account."],
  },
  meta: {
    idLabel: "App ID",
    secretLabel: "App secret",
    link: "https://developers.facebook.com/apps",
    steps: [
      "On developers.facebook.com create a Business app and add Facebook Login for Business.",
      "Add the redirect URL below under Valid OAuth Redirect URIs. Localhost works while the app is in Development mode, which is enough for your own accounts.",
      "Copy the App ID and App secret from App settings → Basic. Your Instagram must be a Professional account linked to your Facebook Page.",
    ],
    limits: ["One sign-in covers your Facebook Pages and their linked Instagram accounts.", "Facebook sign-ins last about 60 days, then you sign in again.", "The Facebook Reels API takes 3–90 s videos."],
  },
};

function SecretInput({ name, label, isSet, onDone }: { name: string; label: string; isSet: boolean; onDone: (m: Msg) => void }) {
  const [value, setValue] = useState("");
  const save = async () => {
    try {
      await api.setSecret(name, value);
      setValue("");
      onDone({ tone: "good", text: `${label} saved to Windows Credential Manager.` });
    } catch (e) {
      onDone({ tone: "bad", text: errorText(e) });
    }
  };
  return (
    <Field label={label} hint={isSet ? "Saved. Paste a new value to replace it." : undefined}>
      <div className="flex gap-2">
        <input type="password" className={inputCls} value={value} onChange={(e) => setValue(e.target.value)} placeholder={isSet ? "••••••••••••" : "Paste here"} />
        <Button variant="primary" disabled={!value.trim()} onClick={save}>Save</Button>
      </div>
    </Field>
  );
}

function AccountDetails({ p }: { p: Platform }) {
  const d = p.account?.details ?? {};
  if (p.id === "kick" && d.slug) return <div className="text-sm text-ink-300">kick.com/{String(d.slug)}</div>;
  if (p.id === "meta") {
    const pages = (d.pages as { name: string; instagram_username: string | null }[] | undefined) ?? [];
    if (!pages.length) return <div className="text-sm text-amber-300">No Facebook Pages granted. Sign in again and select your Page.</div>;
    return (
      <ul className="space-y-0.5 text-sm text-ink-300">
        {pages.map((pg) => (
          <li key={pg.name}>
            Page <span className="text-ink-100">{pg.name}</span>
            {pg.instagram_username ? <> · Instagram <span className="text-ink-100">@{pg.instagram_username}</span></> : <span className="text-ink-500"> · no Instagram linked</span>}
          </li>
        ))}
      </ul>
    );
  }
  return null;
}

function PlatformCard({ p, busy, onSignIn, onChange }: { p: Platform; busy: string | null; onSignIn: (id: string) => void; onChange: (m: Msg) => void }) {
  const g = GUIDE[p.id];
  const [setupOpen, setSetupOpen] = useState(!p.keys_set);
  const [secretStatus, setSecretStatus] = useState<Record<string, boolean>>({});
  const waiting = busy === p.id;

  useEffect(() => {
    api.secretStatus().then(setSecretStatus);
  }, [p.keys_set]);

  const act = async (f: () => Promise<unknown>, ok: (r: unknown) => string) => {
    try {
      onChange({ tone: "good", text: ok(await f()) });
    } catch (e) {
      onChange({ tone: "bad", text: errorText(e) });
    }
  };

  return (
    <Card
      title={p.name}
      right={
        p.account ? (
          p.account.last_error ? <Badge tone="bad">Needs attention</Badge> : <Badge tone="good"><CheckCircle2 size={12} /> Connected</Badge>
        ) : (
          <Badge>Not connected</Badge>
        )
      }
    >
      {p.account ? (
        <div className="flex items-start justify-between gap-6">
          <div className="min-w-0">
            <div className="text-base font-medium">{p.account.display_name}</div>
            <AccountDetails p={p} />
            <div className="mt-1 text-xs text-ink-500">
              Connected {ago(p.account.connected_at)}
              {p.account.expires_at && ` · sign-in renews until ${new Date(p.account.expires_at).toLocaleDateString()}`}
            </div>
            {p.account.last_error && <div className="mt-1 text-sm text-red-300">{p.account.last_error}</div>}
          </div>
          <div className="flex shrink-0 gap-2">
            <Button onClick={() => act(() => api.checkAccount(p.id), (n) => `${p.name}: still linked as ${n}.`)}><RefreshCw size={14} /> Check</Button>
            <Button onClick={() => onSignIn(p.id)} disabled={!!busy}><LogIn size={14} /> Sign in again</Button>
            <Button variant="danger" onClick={() => act(() => api.disconnect(p.id), () => `${p.name} disconnected.`)}><LogOut size={14} /> Disconnect</Button>
          </div>
        </div>
      ) : waiting ? (
        <div className="flex items-center justify-between gap-4">
          <span className="text-sm text-ink-300">Waiting for you to approve in your browser…</span>
          <Button onClick={() => api.cancelSignIn()}>Cancel</Button>
        </div>
      ) : (
        <div className="flex items-center justify-between gap-4">
          <span className="text-sm text-ink-300">{p.keys_set ? "Ready. Sign in once and StreamForge keeps you connected." : "Finish the one-time app setup below, then sign in."}</span>
          <Button variant="primary" disabled={!p.keys_set || !!busy} onClick={() => onSignIn(p.id)}>
            <LogIn size={15} /> Sign in with {p.name}
          </Button>
        </div>
      )}

      <div className="mt-4 border-t border-ink-800 pt-3">
        <button className="flex w-full items-center justify-between text-sm text-ink-300 hover:text-ink-100" onClick={() => setSetupOpen(!setupOpen)}>
          <span className="flex items-center gap-2"><KeyRound size={14} /> Developer app (one-time setup) {p.keys_set && <Badge tone="good">done</Badge>}</span>
          <ChevronDown size={15} className={`transition-transform ${setupOpen ? "rotate-180" : ""}`} />
        </button>
        {setupOpen && (
          <div className="mt-4 grid grid-cols-2 gap-6">
            <div className="space-y-3 text-sm text-ink-300">
              <ol className="list-decimal space-y-1.5 pl-5">{g.steps.map((s, i) => <li key={i}>{s}</li>)}</ol>
              {p.id !== "youtube" && (
                <div className="flex items-center gap-2 rounded-lg bg-ink-950 px-3 py-2">
                  <code className="flex-1 truncate text-ink-100">{p.redirect_uri}</code>
                  <button title="Copy" className="hover:text-white" onClick={() => navigator.clipboard.writeText(p.redirect_uri).then(() => onChange({ tone: "info", text: "Redirect URL copied." }))}>
                    <Copy size={14} />
                  </button>
                </div>
              )}
              <button className="inline-flex items-center gap-1 text-sky-300 hover:underline" onClick={() => openUrl(g.link)}>
                Open {p.name} developer site <ExternalLink size={13} />
              </button>
              <ul className="list-disc space-y-1 pl-5 text-xs text-ink-500">{g.limits.map((l) => <li key={l}>{l}</li>)}</ul>
            </div>
            <div className="space-y-4">
              <SecretInput name={p.id_key} label={g.idLabel} isSet={!!secretStatus[p.id_key]} onDone={onChange} />
              <SecretInput name={p.secret_key} label={g.secretLabel} isSet={!!secretStatus[p.secret_key]} onDone={onChange} />
            </div>
          </div>
        )}
      </div>
    </Card>
  );
}

export default function Connections() {
  const [platforms, setPlatforms] = useState<Platform[]>([]);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [slug, setSlug] = useState("");
  const [msg, setMsg] = useState<Msg>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const load = async () => {
    setPlatforms(await api.platforms());
    const s = await api.settings();
    setSettings(s);
    setSlug(s.kick_channel_slug ?? "");
  };
  useEffect(() => {
    load().catch((e) => setMsg({ tone: "bad", text: errorText(e) }));
  }, []);

  const onChange = (m: Msg) => {
    setMsg(m);
    load();
  };

  const signIn = async (id: string) => {
    setBusy(id);
    setMsg({ tone: "info", text: "Your browser opened. Approve access there and come back." });
    try {
      const name = await api.signIn(id);
      onChange({ tone: "good", text: `Signed in as ${name}.` });
    } catch (e) {
      onChange({ tone: "bad", text: errorText(e) });
    } finally {
      setBusy(null);
    }
  };

  const saveSlug = async () => {
    if (!settings) return;
    try {
      const s = await api.saveSettings({ ...settings, kick_channel_slug: slug || null });
      setSettings(s);
      setSlug(s.kick_channel_slug ?? "");
      setMsg({ tone: "good", text: s.kick_channel_slug ? `Monitoring kick.com/${s.kick_channel_slug}.` : "Channel cleared." });
    } catch (e) {
      setMsg({ tone: "bad", text: errorText(e) });
    }
  };

  const kickKeys = platforms.find((p) => p.id === "kick")?.keys_set;

  return (
    <Page title="Account Connections" subtitle="Sign in once per platform. Tokens are kept in Windows Credential Manager and renewed automatically.">
      {msg && (
        <div className="sticky top-0 z-10 mb-4">
          <Notice tone={msg.tone}>{msg.text}</Notice>
        </div>
      )}
      <div className="space-y-4">
        {platforms.map((p) => (
          <PlatformCard key={p.id} p={p} busy={busy} onSignIn={signIn} onChange={onChange} />
        ))}

        <Card title="Kick live monitoring">
          <div className="flex items-end gap-4">
            <div className="flex-1">
              <Field label="Channel to watch" hint="Filled in automatically when you sign in to Kick. A kick.com URL works too.">
                <input className={inputCls} value={slug} onChange={(e) => setSlug(e.target.value)} placeholder="yourchannel" />
              </Field>
            </div>
            <Button variant="primary" onClick={saveSlug}>Save</Button>
            <Button
              disabled={!kickKeys || !settings?.kick_channel_slug}
              onClick={async () => {
                try {
                  setMsg({ tone: "good", text: await api.testKick() });
                } catch (e) {
                  setMsg({ tone: "bad", text: errorText(e) });
                }
              }}
            >
              Test
            </Button>
          </div>
        </Card>
      </div>
    </Page>
  );
}
