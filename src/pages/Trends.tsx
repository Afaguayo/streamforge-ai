import { openUrl } from "@tauri-apps/plugin-opener";
import { Play, Plus, X } from "lucide-react";
import { useEffect, useState } from "react";
import { Badge, Button, Card, Empty, Field, Notice, Page, Stat, inputCls } from "../components/ui";
import { api, errorText, type Settings } from "../lib/api";
import { ago, duration } from "../lib/format";
import { usePoll } from "../lib/usePoll";

export default function Trends() {
  const { data: r, refresh } = usePoll(api.trendReport, 8000);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [kw, setKw] = useState("");
  const [msg, setMsg] = useState<{ text: string; bad?: boolean } | null>(null);
  useEffect(() => { api.settings().then(setSettings); }, []);

  const saveKeywords = async (list: string[]) => {
    if (!settings) return;
    try {
      setSettings(await api.saveSettings({ ...settings, trend_keywords: list }));
    } catch (e) {
      setMsg({ text: errorText(e), bad: true });
    }
  };
  const run = async () => {
    try {
      await api.runTrendReport();
      setMsg({ text: "Researching… the report appears here in a minute." });
      setTimeout(refresh, 15000);
    } catch (e) {
      setMsg({ text: errorText(e), bad: true });
    }
  };
  const rep = r?.report;
  const t = rep?.title_traits ?? {};

  return (
    <Page title="Trend Research" subtitle="What the most-viewed YouTube videos for your keywords did this week. Refreshes daily; feeds clip scoring and hashtags.">
      {msg && <div className="mb-4"><Notice tone={msg.bad ? "bad" : "info"}>{msg.text}</Notice></div>}
      <Card title="Your niche">
        <div className="flex flex-wrap items-center gap-2">
          {settings?.trend_keywords.map((k) => (
            <span key={k} className="inline-flex items-center gap-1 rounded-md bg-ink-800 px-2 py-1 text-sm">
              {k}
              <button className="text-ink-500 hover:text-white" onClick={() => saveKeywords(settings.trend_keywords.filter((x) => x !== k))}><X size={13} /></button>
            </span>
          ))}
        </div>
        <div className="mt-3 flex items-end gap-2">
          <div className="flex-1">
            <Field label="Add a keyword" hint="Up to 5. E.g. “valorant clips”, “just chatting español”. Each search uses ~100 of your 10,000 daily YouTube API units.">
              <input className={inputCls} value={kw} onChange={(e) => setKw(e.target.value)} onKeyDown={(e) => { if (e.key === "Enter" && kw.trim() && settings) { saveKeywords([...settings.trend_keywords, kw]); setKw(""); } }} />
            </Field>
          </div>
          <Button disabled={!kw.trim() || !settings} onClick={() => { if (settings) { saveKeywords([...settings.trend_keywords, kw]); setKw(""); } }}><Plus size={14} /> Add</Button>
          <Button variant="primary" disabled={!settings?.trend_keywords.length} onClick={run}><Play size={14} /> Run report now</Button>
        </div>
      </Card>

      {!rep ? (
        <div className="mt-5"><Empty>No report yet. Add keywords and run one (YouTube must be connected).</Empty></div>
      ) : (
        <div className="mt-5 space-y-5">
          <div className="text-sm text-ink-500">Report for “{r!.keywords}”, {ago(r!.created_at)} · {rep.videos_analyzed} videos from the last 7 days</div>
          <div className="grid grid-cols-4 gap-4">
            <Stat label="Median views" value={rep.median_views.toLocaleString()} />
            <Stat label="Engagement" value={`${rep.avg_engagement_pct}%`} hint="likes + comments per view" />
            <Stat label="Short-form" value={`${rep.pct_short_form}%`} hint="of top videos are ≤ 3 min" />
            <Stat label="Best upload hour" value={`${String((rep.top_publish_hour_utc - new Date().getTimezoneOffset() / 60 + 24) % 24).padStart(2, "0")}:00`} hint="your time" />
          </div>
          <div className="grid grid-cols-3 gap-5">
            <Card title="Title patterns">
              <ul className="space-y-1.5 text-sm text-ink-300">
                <li>Average length: <span className="text-ink-100">{t.avg_title_chars} characters</span></li>
                <li>Use an ALL-CAPS word: <span className="text-ink-100">{t.pct_with_caps_word}%</span></li>
                <li>Ask a question: <span className="text-ink-100">{t.pct_questions}%</span></li>
                <li>Contain a number: <span className="text-ink-100">{t.pct_numbers}%</span></li>
                <li>Use emoji: <span className="text-ink-100">{t.pct_emoji}%</span></li>
              </ul>
            </Card>
            <Card title="Frequent title words">
              <div className="flex flex-wrap gap-1.5">{rep.top_words.map(([w, n]) => <Badge key={w}>{w} · {n}</Badge>)}</div>
            </Card>
            <Card title="Hashtags in use">
              <div className="flex flex-wrap gap-1.5">{rep.top_hashtags.length ? rep.top_hashtags.map(([w, n]) => <Badge key={w} tone="info">#{w} · {n}</Badge>) : <span className="text-sm text-ink-500">None repeated.</span>}</div>
            </Card>
          </div>
          <Card title="Top videos this week">
            <div className="grid grid-cols-4 gap-4">
              {rep.top_videos.map((v) => (
                <button key={v.id} className="text-left" onClick={() => openUrl(`https://youtu.be/${v.id}`)}>
                  <img src={v.thumb} className="aspect-video w-full rounded-md object-cover" />
                  <div className="mt-1.5 line-clamp-2 text-sm">{v.title}</div>
                  <div className="text-xs text-ink-500">{v.channel} · {v.views.toLocaleString()} views · {duration(v.seconds)}</div>
                </button>
              ))}
            </div>
            <p className="mt-3 text-xs text-ink-500">Thumbnails are shown for reference only; StreamForge never copies or downloads other creators' content.</p>
          </Card>
        </div>
      )}
    </Page>
  );
}
