import { useEffect, useState } from "react";
import { PlatformCard } from "../components/PlatformCard";
import { Button, Card, Field, Notice, Page, inputCls } from "../components/ui";
import { api, errorText, type Platform, type Settings } from "../lib/api";

export default function Connections() {
  const [platforms, setPlatforms] = useState<Platform[]>([]);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [slug, setSlug] = useState("");
  const [msg, setMsg] = useState<{ tone: "good" | "bad"; text: string } | null>(null);

  const load = async () => {
    setPlatforms(await api.platforms());
    const s = await api.settings();
    setSettings(s);
    setSlug(s.kick_channel_slug ?? "");
  };
  useEffect(() => {
    load().catch((e) => setMsg({ tone: "bad", text: errorText(e) }));
  }, []);

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
    try {
      setMsg({ tone: "good", text: await api.testKick() });
    } catch (e) {
      setMsg({ tone: "bad", text: errorText(e) });
    }
  };

  return (
    <Page title="Account Connections" subtitle="Sign in once per platform. Logins are kept in Windows Credential Manager and renewed automatically.">
      <div className="space-y-4">
        {platforms.map((p) => (
          <PlatformCard key={p.id} p={p} onChanged={load} />
        ))}

        <Card title="Kick live monitoring">
          <div className="flex items-end gap-3">
            <div className="flex-1">
              <Field label="Channel to watch" hint="Filled in automatically when you sign in to Kick. A kick.com link works too.">
                <input className={inputCls} value={slug} onChange={(e) => setSlug(e.target.value)} placeholder="yourchannel" />
              </Field>
            </div>
            <Button variant="primary" onClick={saveSlug}>Save</Button>
            <Button onClick={test}>Test</Button>
          </div>
          {msg && (
            <div className="mt-3">
              <Notice tone={msg.tone}>{msg.text}</Notice>
            </div>
          )}
        </Card>
      </div>
    </Page>
  );
}
