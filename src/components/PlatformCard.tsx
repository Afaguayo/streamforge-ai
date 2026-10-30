import { openUrl } from "@tauri-apps/plugin-opener";
import { CheckCircle2, ChevronDown, Copy, ExternalLink, KeyRound, LogIn, LogOut, RefreshCw } from "lucide-react";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { api, errorText, type Platform } from "../lib/api";
import { ago } from "../lib/format";
import { Badge, Button, Card, Field, Notice, inputCls } from "./ui";

type Msg = { tone: "good" | "bad" | "info" | "warn"; text: string } | null;

const Link = ({ href, children }: { href: string; children: ReactNode }) => (
  <button className="inline-flex items-center gap-0.5 text-sky-300 hover:underline" onClick={() => openUrl(href)}>
    {children} <ExternalLink size={11} />
  </button>
);
const K = ({ children }: { children: ReactNode }) => <span className="font-medium text-ink-100">{children}</span>;

// One-time developer-app setup per platform. Full walkthrough with screenshots: docs/SETUP.md.
export const GUIDE: Record<Platform["id"], { idLabel: string; secretLabel: string; minutes: number; steps: ReactNode[]; limits: string[] }> = {
  kick: {
    idLabel: "Client ID",
    secretLabel: "Client Secret",
    minutes: 3,
    steps: [
      <>Turn on two-factor authentication on Kick if it isn't already (<Link href="https://kick.com/settings/security">Security settings</Link>).</>,
      <>Open <Link href="https://kick.com/settings/developer">Settings → Developer</Link> and press <K>Create new app</K>.</>,
      <>Name it <K>StreamForge</K>, paste the <K>redirect URL</K> shown below, and tick the scopes <K>user:read</K> and <K>channel:read</K>.</>,
      <>Save, then copy the <K>Client ID</K> and <K>Client Secret</K> into the boxes on the right.</>,
    ],
    limits: ["Kick's public API has no VOD downloads, so recordings come from your OBS recording folder."],
  },
  youtube: {
    idLabel: "Client ID",
    secretLabel: "Client Secret",
    minutes: 6,
    steps: [
      <>Open <Link href="https://console.cloud.google.com/projectcreate">Google Cloud → New project</Link>, name it <K>StreamForge</K> and create it.</>,
      <>Enable <Link href="https://console.cloud.google.com/apis/library/youtube.googleapis.com">YouTube Data API v3</Link> and <Link href="https://console.cloud.google.com/apis/library/youtubeanalytics.googleapis.com">YouTube Analytics API</Link> (press <K>Enable</K> on each).</>,
      <>Open <Link href="https://console.cloud.google.com/auth/overview">Google Auth Platform</Link> → <K>Get started</K>: app name StreamForge, your email, audience <K>External</K>.</>,
      <>Go to <Link href="https://console.cloud.google.com/auth/audience">Audience</Link> and press <K>Publish app</K> (or add your Gmail under Test users; then sign-ins expire after 7 days).</>,
      <>Go to <Link href="https://console.cloud.google.com/auth/clients">Clients</Link> → <K>Create client</K> → type <K>Desktop app</K>, then copy the <K>Client ID</K> and <K>Client secret</K> into the boxes on the right.</>,
    ],
    limits: [
      "Google shows an \"app isn't verified\" screen when you sign in. Press Advanced → Go to StreamForge; it's your own app.",
      "Uploads stay private until Google audits the API project. The default quota allows about 6 uploads a day.",
    ],
  },
  tiktok: {
    idLabel: "Client key",
    secretLabel: "Client secret",
    minutes: 8,
    steps: [
      <>Log in at <Link href="https://developers.tiktok.com/apps">TikTok for Developers → Manage apps</Link> and press <K>Connect an app</K>. In the app details use Terms of Service <K>https://afaguayo.github.io/streamforge-ai/terms.html</K>, Privacy Policy <K>https://afaguayo.github.io/streamforge-ai/privacy.html</K>, tick platform <K>Desktop</K>, and Web/Desktop URL <K>https://afaguayo.github.io/streamforge-ai/</K>.</>,
      <>Open the app's <K>Sandbox</K> tab, create a sandbox and add your TikTok account under <K>Target users</K>. This works without TikTok's review.</>,
      <>Add the products <K>Login Kit</K> and <K>Content Posting API</K>. In Login Kit choose platform <K>Desktop</K> and paste the <K>redirect URL</K> shown below.</>,
      <>Under Scopes make sure <K>user.info.basic</K>, <K>video.upload</K> and <K>video.publish</K> are on, then copy the <K>Client key</K> and <K>Client secret</K> into the boxes on the right.</>,
    ],
    limits: ["Until TikTok audits your app, posts can only go to a private account."],
  },
  meta: {
    idLabel: "App ID",
    secretLabel: "App secret",
    minutes: 8,
    steps: [
      <>Make your Instagram a <K>Professional</K> account and link it to your Facebook Page (Instagram app → Settings → Account type and tools).</>,
      <>Open <Link href="https://developers.facebook.com/apps/creation/">Meta for Developers → Create app</Link>. Pick the use cases <K>Manage everything on your Page</K> and <K>Manage messaging & content on Instagram</K>.</>,
      <>In the app open <K>Facebook Login for Business → Settings</K> and paste the <K>redirect URL</K> shown below into <K>Valid OAuth Redirect URIs</K>. Save.</>,
      <>Open <K>App settings → Basic</K> and copy the <K>App ID</K> and <K>App secret</K> into the boxes on the right. Leave the app in <K>Development</K> mode; that's enough for your own accounts.</>,
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
      onDone({ tone: "good", text: `${label} saved.` });
    } catch (e) {
      onDone({ tone: "bad", text: errorText(e) });
    }
  };
  return (
    <Field label={label} hint={isSet ? "✓ Saved. Paste a new value to replace it." : undefined}>
      <div className="flex gap-2">
        <input
          type="password"
          className={inputCls}
          value={value}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && value.trim() && save()}
          placeholder={isSet ? "••••••••••••" : "Paste here"}
        />
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

/** Sign-in card for one platform: status, sign-in/out, and the one-time developer-app setup. */
export function PlatformCard({ p, onChanged }: { p: Platform; onChanged: () => void }) {
  const g = GUIDE[p.id];
  const [setupOpen, setSetupOpen] = useState(!p.keys_set && !p.account);
  const [secretStatus, setSecretStatus] = useState<Record<string, boolean>>({});
  const [msg, setMsg] = useState<Msg>(null);
  const [waiting, setWaiting] = useState(false);
  const setupRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    api.secretStatus().then(setSecretStatus);
  }, [p.keys_set]);

  const done = (m: Msg) => {
    setMsg(m);
    api.secretStatus().then(setSecretStatus);
    onChanged();
  };

  const act = async (f: () => Promise<unknown>, ok: (r: unknown) => string) => {
    try {
      done({ tone: "good", text: ok(await f()) });
    } catch (e) {
      done({ tone: "bad", text: errorText(e) });
    }
  };

  const signIn = async () => {
    if (!p.keys_set) {
      // No app keys yet: show the setup steps instead of failing.
      setSetupOpen(true);
      setMsg({ tone: "warn", text: `First paste your ${p.name} app's ${g.idLabel} and ${g.secretLabel} below (one-time setup, about ${g.minutes} min), then press Sign in again.` });
      setTimeout(() => {
        setupRef.current?.scrollIntoView({ behavior: "smooth", block: "center" });
        setupRef.current?.querySelector("input")?.focus();
      }, 50);
      return;
    }
    setWaiting(true);
    setMsg({ tone: "info", text: "Your browser opened. Approve access there, then come back here." });
    try {
      const name = await api.signIn(p.id);
      done({ tone: "good", text: `Signed in as ${name}.` });
    } catch (e) {
      done({ tone: "bad", text: errorText(e) });
    } finally {
      setWaiting(false);
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
            <Button onClick={() => act(() => api.checkAccount(p.id), (n) => `Still linked as ${n}.`)}><RefreshCw size={14} /> Check</Button>
            <Button onClick={signIn} disabled={waiting}><LogIn size={14} /> Sign in again</Button>
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
          <span className="text-sm text-ink-300">
            {p.keys_set ? "Ready. Sign in once and StreamForge keeps you connected." : `Needs a one-time app setup first (about ${g.minutes} min).`}
          </span>
          <Button variant="primary" onClick={signIn}>
            <LogIn size={15} /> Sign in with {p.name}
          </Button>
        </div>
      )}

      {msg && (
        <div className="mt-3">
          <Notice tone={msg.tone}>{msg.text}</Notice>
        </div>
      )}

      <div className="mt-4 border-t border-ink-800 pt-3">
        <button className="flex w-full items-center justify-between text-sm text-ink-300 hover:text-ink-100" onClick={() => setSetupOpen(!setupOpen)}>
          <span className="flex items-center gap-2">
            <KeyRound size={14} /> One-time app setup {p.keys_set ? <Badge tone="good">done</Badge> : <span className="text-ink-500">· about {g.minutes} min</span>}
          </span>
          <ChevronDown size={15} className={`transition-transform ${setupOpen ? "rotate-180" : ""}`} />
        </button>
        {setupOpen && (
          <div ref={setupRef} className="mt-4 grid grid-cols-5 gap-6">
            <div className="col-span-3 space-y-3 text-sm text-ink-300">
              <ol className="space-y-2.5">
                {g.steps.map((s, i) => (
                  <li key={i} className="flex gap-3">
                    <span className="flex h-5 w-5 shrink-0 items-center justify-center rounded-full bg-ink-800 text-[11px] font-semibold text-ink-100">{i + 1}</span>
                    <span className="leading-relaxed">{s}</span>
                  </li>
                ))}
              </ol>
              {p.id !== "youtube" && (
                <div>
                  <div className="mb-1 text-xs uppercase tracking-wider text-ink-500">Redirect URL</div>
                  <div className="flex items-center gap-2 rounded-lg border border-ink-700 bg-ink-950 px-3 py-2">
                    <code className="flex-1 truncate text-ink-100">{p.redirect_uri}</code>
                    <Button onClick={() => navigator.clipboard.writeText(p.redirect_uri).then(() => setMsg({ tone: "info", text: "Redirect URL copied." }))}>
                      <Copy size={13} /> Copy
                    </Button>
                  </div>
                </div>
              )}
              <ul className="list-disc space-y-1 pl-5 text-xs text-ink-500">{g.limits.map((l) => <li key={l}>{l}</li>)}</ul>
            </div>
            <div className="col-span-2 space-y-4">
              <SecretInput name={p.id_key} label={g.idLabel} isSet={!!secretStatus[p.id_key]} onDone={done} />
              <SecretInput name={p.secret_key} label={g.secretLabel} isSet={!!secretStatus[p.secret_key]} onDone={done} />
              <p className="text-xs text-ink-500">Stored in Windows Credential Manager, never in files or logs.</p>
            </div>
          </div>
        )}
      </div>
    </Card>
  );
}
