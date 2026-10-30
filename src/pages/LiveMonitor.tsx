import type { PageId } from "../App";
import { StreamStatus } from "../components/StreamBits";
import { Button, Card, Empty, Notice, Page } from "../components/ui";
import { api } from "../lib/api";
import { ago, duration } from "../lib/format";
import { usePoll } from "../lib/usePoll";

export default function LiveMonitor({ go }: { go: (p: PageId) => void }) {
  const { data: ov } = usePoll(api.overview);
  const { data: streams } = usePoll(api.streams, 5000);
  const k = ov?.kick;
  const sessions = (streams ?? []).filter((s) => s.source === "kick");
  const liveFor = k?.live && k.started_at ? (Date.now() - Date.parse(k.started_at)) / 1000 : null;

  return (
    <Page title="Live Stream Monitor" subtitle="Polls Kick's official API for your channel's live status.">
      {!k?.configured ? (
        <Notice tone="warn">
          <div className="flex items-center justify-between gap-4">
            <span>Kick is not connected. Add your Kick app credentials and channel name to start monitoring.</span>
            <Button onClick={() => go("connections")}>Connect Kick</Button>
          </div>
        </Notice>
      ) : (
        <Card>
          <div className="flex items-center gap-6">
            <div className={`flex h-16 w-16 items-center justify-center rounded-2xl ${k.live ? "bg-kick/15" : "bg-ink-800"}`}>
              <span className={`h-4 w-4 rounded-full ${k.live ? "animate-pulse bg-kick" : "bg-ink-500"}`} />
            </div>
            <div className="min-w-0 flex-1">
              <div className="text-xs uppercase tracking-wider text-ink-500">kick.com/{k.slug}</div>
              <div className="mt-1 truncate text-xl font-semibold">{k.live ? k.title || "Live" : "Offline"}</div>
              <div className="mt-1 flex gap-5 text-sm text-ink-300">
                {k.live && <span>{k.viewers ?? 0} viewers</span>}
                {k.live && k.category && <span>{k.category}</span>}
                {liveFor != null && <span>live for {duration(liveFor)}</span>}
                <span>checked {ago(k.last_checked)}</span>
              </div>
              {k.last_error && <div className="mt-2 text-sm text-red-300">{k.last_error}</div>}
            </div>
          </div>
        </Card>
      )}

      <div className="mt-5">
        <Notice>
          Kick's public API has no VOD download endpoint. When your stream ends, StreamForge waits for the recording to appear in your
          recording folder (OBS: Settings → Output → Recording) and attaches it to the Kick session automatically.
        </Notice>
      </div>

      <Card title="Kick sessions" className="mt-5">
        {sessions.length === 0 ? (
          <Empty>No sessions recorded yet. They appear here when StreamForge sees you go live.</Empty>
        ) : (
          <ul className="divide-y divide-ink-800">
            {sessions.map((s) => (
              <li key={s.id} className="flex items-center gap-4 py-3 text-sm">
                <div className="min-w-0 flex-1">
                  <div className="truncate font-medium">{s.title}</div>
                  <div className="text-xs text-ink-500">
                    started {ago(s.kick_started)}
                    {s.kick_ended && s.kick_started && ` · ${duration((Date.parse(s.kick_ended) - Date.parse(s.kick_started)) / 1000)} long`}
                  </div>
                </div>
                <StreamStatus s={s.status} />
              </li>
            ))}
          </ul>
        )}
      </Card>
    </Page>
  );
}
