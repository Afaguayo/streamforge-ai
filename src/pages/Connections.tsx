import { openUrl } from "@tauri-apps/plugin-opener";
import { CheckCircle2, ExternalLink, KeyRound } from "lucide-react";
import { useEffect, useState } from "react";
import { Badge, Button, Card, Field, Notice, Page, inputCls } from "../components/ui";
import { api, errorText, type Settings } from "../lib/api";

type Msg = { tone: "good" | "bad"; text: string } | null;

function SecretInput({ name, label, isSet, onSaved }: { name: string; label: string; isSet: boolean; onSaved: (m: Msg) => void }) {
  const [value, setValue] = useState("");
  const save = async () => {
    try {
      await api.setSecret(name, value);
      setValue("");
      onSaved({ tone: "good", text: `${label} saved to Windows Credential Manager.` });
    } catch (e) {
      onSaved({ tone: "bad", text: errorText(e) });
    }
  };
  const clear = async () => {
    try {
      await api.clearSecret(name);
      onSaved({ tone: "good", text: `${label} removed.` });
    } catch (e) {
      onSaved({ tone: "bad", text: errorText(e) });
    }
  };
  return (
    <Field label={label} hint={isSet ? "Stored securely. Enter a new value to replace it." : "Not set."}>
      <div className="flex gap-2">
        <input type="password" className={inputCls} value={value} onChange={(e) => setValue(e.target.value)} placeholder={isSet ? "••••••••••••" : ""} />
        <Button variant="primary" disabled={!value.trim()} onClick={save}>Save</Button>
        {isSet && <Button variant="danger" onClick={clear}>Remove</Button>}
      </div>
    </Field>
  );
}

const PLATFORMS = [
  {
    name: "YouTube (videos + Shorts)",
    phase: 5,
    notes: [
      "Google Cloud project with YouTube Data API v3 and an OAuth client (Desktop app).",
      "Uploads from an unverified API project are forced to private until Google's compliance audit passes.",
      "Default quota is 10,000 units/day; one upload costs about 1,600, so roughly 6 uploads a day.",
      "Custom thumbnails need a phone-verified channel. Shorts are vertical uploads up to 3 minutes.",
    ],
  },
  {
    name: "TikTok",
    phase: 5,
    notes: [
      "TikTok developer app with the Content Posting API and video.publish scope.",
      "Until TikTok audits the app, posts can only go to private accounts and at most 5 users can post per 24 h.",
      "The longest allowed video comes from the creator_info endpoint per account.",
    ],
  },
  {
    name: "Instagram Reels",
    phase: 5,
    notes: [
      "Instagram Professional (Business or Creator) account and a Meta app passing App Review for instagram_content_publish.",
      "Publishing is rate-limited per rolling 24 h; the API reports the current limit.",
    ],
  },
  {
    name: "Facebook Reels",
    phase: 5,
    notes: [
      "Publishes to a Facebook Page only (not a personal profile); needs a Page access token.",
      "The Reels API accepts 3–90 second videos, so clips longer than 90 s get flagged for a different destination instead of being cut silently.",
    ],
  },
];

export default function Connections() {
  const [status, setStatus] = useState<Record<string, boolean>>({});
  const [settings, setSettings] = useState<Settings | null>(null);
  const [slug, setSlug] = useState("");
  const [msg, setMsg] = useState<Msg>(null);
  const [testing, setTesting] = useState(false);

  const load = async () => {
    setStatus(await api.secretStatus());
    const s = await api.settings();
    setSettings(s);
    setSlug(s.kick_channel_slug ?? "");
  };
  useEffect(() => {
    load().catch((e) => setMsg({ tone: "bad", text: errorText(e) }));
  }, []);

  const onSaved = (m: Msg) => {
    setMsg(m);
    load();
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

  const test = async () => {
    setTesting(true);
    try {
      setMsg({ tone: "good", text: await api.testKick() });
    } catch (e) {
      setMsg({ tone: "bad", text: errorText(e) });
    } finally {
      setTesting(false);
    }
  };

  const kickReady = status["kick.client_id"] && status["kick.client_secret"] && !!settings?.kick_channel_slug;

  return (
    <Page title="Account Connections" subtitle="Credentials are stored in Windows Credential Manager, never in the database or logs.">
      {msg && (
        <div className="mb-4">
          <Notice tone={msg.tone}>{msg.text}</Notice>
        </div>
      )}

      <Card
        title="Kick"
        right={kickReady ? <Badge tone="good"><CheckCircle2 size={12} /> Configured</Badge> : <Badge tone="warn">Not configured</Badge>}
      >
        <div className="grid grid-cols-2 gap-8">
          <div className="space-y-5">
            <Field label="Channel name" hint="The part after kick.com/ — a full URL works too.">
              <div className="flex gap-2">
                <input className={inputCls} value={slug} onChange={(e) => setSlug(e.target.value)} placeholder="yourchannel" />
                <Button variant="primary" onClick={saveSlug}>Save</Button>
              </div>
            </Field>
            <SecretInput name="kick.client_id" label="Client ID" isSet={!!status["kick.client_id"]} onSaved={onSaved} />
            <SecretInput name="kick.client_secret" label="Client Secret" isSet={!!status["kick.client_secret"]} onSaved={onSaved} />
            <Button disabled={!kickReady || testing} onClick={test}>
              <KeyRound size={15} /> {testing ? "Testing…" : "Test connection"}
            </Button>
          </div>
          <div className="space-y-3 text-sm text-ink-300">
            <p className="font-medium text-ink-100">How to get these</p>
            <ol className="list-decimal space-y-1.5 pl-5">
              <li>On kick.com open Settings → Developer and create an app (2FA must be on).</li>
              <li>Any redirect URL works for now, e.g. <code className="text-ink-100">http://localhost:1420/callback</code>; live monitoring uses an app token, not your login.</li>
              <li>Paste the Client ID and Client Secret here, save your channel name, then press Test connection.</li>
            </ol>
            <button className="inline-flex items-center gap-1 text-sky-300 hover:underline" onClick={() => openUrl("https://docs.kick.com/getting-started/kick-apps-setup")}>
              Kick app setup docs <ExternalLink size={13} />
            </button>
          </div>
        </div>
      </Card>

      <h2 className="mb-3 mt-8 text-sm font-medium uppercase tracking-wider text-ink-500">Publishing destinations</h2>
      <div className="grid grid-cols-2 gap-4">
        {PLATFORMS.map((p) => (
          <Card key={p.name} title={p.name} right={<Badge>Phase {p.phase} · not built yet</Badge>}>
            <ul className="list-disc space-y-1.5 pl-5 text-sm text-ink-300">
              {p.notes.map((n) => (
                <li key={n}>{n}</li>
              ))}
            </ul>
          </Card>
        ))}
      </div>
    </Page>
  );
}
