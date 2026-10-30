import { RefreshCw } from "lucide-react";
import { useState } from "react";
import { Button, Card, Empty, Notice, Page, Stat } from "../components/ui";
import { api, errorText } from "../lib/api";
import { ago } from "../lib/format";
import { usePoll } from "../lib/usePoll";

const HUE = "#38bdf8"; // single sequential hue for every magnitude chart
const fmt = (n: number) => (n >= 1e6 ? `${(n / 1e6).toFixed(1)}M` : n >= 1e3 ? `${(n / 1e3).toFixed(1)}K` : String(Math.round(n)));

/** Views over time: one line, crosshair + tooltip on hover. */
function LineChart({ points }: { points: [string, number][] }) {
  const [hover, setHover] = useState<number | null>(null);
  const W = 720, H = 220, P = { l: 44, r: 12, t: 12, b: 26 };
  const max = Math.max(1, ...points.map((p) => p[1]));
  const x = (i: number) => P.l + (points.length === 1 ? (W - P.l - P.r) / 2 : (i / (points.length - 1)) * (W - P.l - P.r));
  const y = (v: number) => P.t + (1 - v / max) * (H - P.t - P.b);
  const path = points.map((p, i) => `${i ? "L" : "M"}${x(i)},${y(p[1])}`).join(" ");
  return (
    <svg viewBox={`0 0 ${W} ${H}`} className="w-full" onMouseLeave={() => setHover(null)}>
      {[0, 0.5, 1].map((f) => (
        <g key={f}>
          <line x1={P.l} x2={W - P.r} y1={y(max * f)} y2={y(max * f)} stroke="#252b37" strokeWidth={1} />
          <text x={P.l - 8} y={y(max * f) + 4} textAnchor="end" fontSize={11} fill="#5b6475">{fmt(max * f)}</text>
        </g>
      ))}
      <path d={path} fill="none" stroke={HUE} strokeWidth={2} strokeLinejoin="round" />
      {points.map((p, i) => (
        <g key={p[0]}>
          <rect x={x(i) - 18} y={P.t} width={36} height={H - P.t - P.b} fill="transparent" onMouseEnter={() => setHover(i)} />
          {(points.length < 20 || hover === i) && <circle cx={x(i)} cy={y(p[1])} r={hover === i ? 5 : 4} fill={HUE} stroke="#0d0f14" strokeWidth={2} />}
        </g>
      ))}
      {points.length > 0 && [0, points.length - 1].map((i) => (
        <text key={i} x={x(i)} y={H - 6} textAnchor={i ? "end" : "start"} fontSize={11} fill="#5b6475">{points[i][0]}</text>
      ))}
      {hover != null && (
        <g pointerEvents="none">
          <line x1={x(hover)} x2={x(hover)} y1={P.t} y2={H - P.b} stroke="#5b6475" strokeDasharray="3 3" />
          <rect x={Math.min(x(hover) + 8, W - 140)} y={P.t} width={130} height={40} rx={6} fill="#181c25" stroke="#252b37" />
          <text x={Math.min(x(hover) + 18, W - 130)} y={P.t + 16} fontSize={11} fill="#a3abba">{points[hover][0]}</text>
          <text x={Math.min(x(hover) + 18, W - 130)} y={P.t + 32} fontSize={13} fill="#e6e9ef">{points[hover][1].toLocaleString()} views</text>
        </g>
      )}
    </svg>
  );
}

/** Horizontal bars, one hue, value labels at the bar end, tooltip with detail. */
function Bars({ rows, unit }: { rows: { label: string; value: number; detail: string }[]; unit: string }) {
  const max = Math.max(1, ...rows.map((r) => r.value));
  return (
    <div className="space-y-2">
      {rows.map((r) => (
        <div key={r.label} className="group grid grid-cols-[110px_1fr_70px] items-center gap-3 text-sm" title={`${r.label}: ${r.value.toLocaleString()} ${unit} (${r.detail})`}>
          <span className="truncate text-ink-300">{r.label}</span>
          <div className="h-3 rounded-r bg-ink-800/40">
            <div className="h-full rounded-r transition-opacity group-hover:opacity-80" style={{ width: `${(r.value / max) * 100}%`, background: HUE }} />
          </div>
          <span className="text-right tabular-nums text-ink-100">{fmt(r.value)}</span>
        </div>
      ))}
    </div>
  );
}

export default function Analytics() {
  const { data: a, refresh } = usePoll(api.analytics, 15000);
  const [msg, setMsg] = useState<{ text: string; bad?: boolean } | null>(null);
  const [table, setTable] = useState(false);

  const refreshNow = async () => {
    try {
      await api.refreshStats();
      setMsg({ text: "Fetching the latest numbers from YouTube…" });
      setTimeout(refresh, 6000);
    } catch (e) {
      setMsg({ text: errorText(e), bad: true });
    }
  };

  return (
    <Page
      title="Analytics"
      subtitle={`Real numbers from the YouTube Data API for your published posts.${a?.last_refresh ? ` Last updated ${ago(a.last_refresh)}.` : ""}`}
      actions={<Button onClick={refreshNow}><RefreshCw size={14} /> Refresh now</Button>}
    >
      {msg && <div className="mb-4"><Notice tone={msg.bad ? "bad" : "info"}>{msg.text}</Notice></div>}
      {!a || a.published === 0 ? (
        <Empty>No published posts yet. Numbers appear here once something is live; nothing is estimated.</Empty>
      ) : (
        <div className="space-y-5">
          <div className="grid grid-cols-4 gap-4">
            <Stat label="Published" value={a.published} />
            <Stat label="Views" value={a.total_views.toLocaleString()} />
            <Stat label="Likes" value={a.total_likes.toLocaleString()} />
            <Stat label="Comments" value={a.total_comments.toLocaleString()} />
          </div>
          <Card title="Total views over time" right={<Button onClick={() => setTable(!table)}>{table ? "Chart" : "Table"}</Button>}>
            {a.series.length === 0 ? (
              <Empty>Views are recorded every 6 hours once posts are published.</Empty>
            ) : table ? (
              <table className="w-full text-sm">
                <thead className="text-left text-xs text-ink-500"><tr><th className="pb-2 font-medium">Day</th><th className="pb-2 text-right font-medium">Views</th></tr></thead>
                <tbody>{a.series.map(([d, v]) => <tr key={d} className="border-t border-ink-800"><td className="py-1.5">{d}</td><td className="py-1.5 text-right tabular-nums">{v.toLocaleString()}</td></tr>)}</tbody>
              </table>
            ) : (
              <LineChart points={a.series} />
            )}
          </Card>
          <div className="grid grid-cols-3 gap-5">
            <Card title="Views by platform">
              <Bars unit="views" rows={a.by_platform.map(([p, n, v]) => ({ label: p === "youtube" ? "YouTube" : "TikTok", value: v, detail: `${n} posts` }))} />
              {a.by_platform.some(([p]) => p === "tiktok") && <p className="mt-3 text-xs text-ink-500">TikTok stats need the app to pass TikTok's audit; private sandbox posts report none.</p>}
            </Card>
            <Card title="Avg views by clip type">
              {a.by_category.length === 0 ? <p className="text-sm text-ink-500">Appears once published clips have stats.</p> : <Bars unit="avg views" rows={a.by_category.map(([c, v, n]) => ({ label: c, value: v, detail: `${n} clips` }))} />}
              <p className="mt-3 text-xs text-ink-500">Types with 3+ clips nudge future clip scores up to +20% / −15%.</p>
            </Card>
            <Card title="Avg views by posting hour">
              {a.by_hour.length === 0 ? <p className="text-sm text-ink-500">No data yet.</p> : <Bars unit="avg views" rows={a.by_hour.map(([h, v, n]) => ({ label: `${String((h - new Date().getTimezoneOffset() / 60 + 24) % 24).padStart(2, "0")}:00`, value: v, detail: `${n} posts` }))} />}
              <p className="mt-3 text-xs text-ink-500">Hours in your time zone.</p>
            </Card>
          </div>
        </div>
      )}
    </Page>
  );
}
