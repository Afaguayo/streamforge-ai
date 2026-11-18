import { Search } from "lucide-react";
import { useMemo, useState } from "react";
import { Badge, Card, Empty, Notice, Page, Tabs, inputCls, type Tone } from "../components/ui";
import { api, type Activity } from "../lib/api";
import { ago } from "../lib/format";
import { usePoll } from "../lib/usePoll";

/** Plain names for the `area` each log line was written from. */
const AREA: Record<string, string> = {
  app: "App",
  kick: "Kick",
  ingest: "Recordings",
  pipeline: "Processing",
  settings: "Settings",
  credentials: "Keys",
  accounts: "Accounts",
  render: "Clips",
  publish: "Publishing",
  posts: "Publishing",
  scheduler: "Schedule",
  research: "Research",
  censor: "Censor",
};
export const areaName = (s: string) => AREA[s] ?? s.charAt(0).toUpperCase() + s.slice(1);

const LEVEL: Record<Activity["level"], { label: string; tone: Tone }> = {
  error: { label: "Problem", tone: "bad" },
  warn: { label: "Warning", tone: "warn" },
  info: { label: "Info", tone: "neutral" },
};

type Filter = "all" | "error" | "warn" | "info";

/** One log line: when, what area, what happened. */
export function ActivityRow({ a }: { a: Activity }) {
  const lv = LEVEL[a.level] ?? LEVEL.info;
  return (
    <li className="flex items-start gap-3 py-2">
      <div className="w-20 shrink-0 pt-0.5"><Badge tone={lv.tone}>{lv.label}</Badge></div>
      <div className="min-w-0 flex-1">
        <div className={`break-words text-sm leading-snug ${a.level === "error" ? "text-red-200" : a.level === "warn" ? "text-amber-200" : "text-ink-100"}`}>{a.message}</div>
        <div className="mt-0.5 text-xs text-ink-400" title={new Date(a.ts).toLocaleString()}>{areaName(a.source)} · {ago(a.ts)}</div>
      </div>
    </li>
  );
}

export default function ActivityPage() {
  const { data, error } = usePoll(() => api.activityLog(500), 3000);
  const [level, setLevel] = useState<Filter>("all");
  const [area, setArea] = useState("");
  const [q, setQ] = useState("");

  const all = data ?? [];
  const areas = useMemo(() => [...new Set(all.map((a) => a.source))].sort(), [all]);
  const count = (l: Filter) => (l === "all" ? all.length : all.filter((a) => a.level === l).length);
  const shown = all.filter((a) => (level === "all" || a.level === level) && (!area || a.source === area) && (!q.trim() || a.message.toLowerCase().includes(q.trim().toLowerCase())));

  return (
    <Page title="Activity log" subtitle="Everything StreamForge did and anything that went wrong, newest first. Updates every few seconds. Shows the latest 500 entries.">
      {error && <div className="mb-4"><Notice tone="bad">Could not load the log: {error}</Notice></div>}
      <div className="mb-4 flex flex-wrap items-center gap-3">
        <Tabs<Filter>
          value={level}
          onChange={setLevel}
          tabs={[
            { id: "all", label: "Everything", count: count("all") },
            { id: "error", label: "Problems", count: count("error") },
            { id: "warn", label: "Warnings", count: count("warn") },
            { id: "info", label: "Info", count: count("info") },
          ]}
        />
        <select aria-label="Filter by area" className={`${inputCls} w-44`} value={area} onChange={(e) => setArea(e.target.value)}>
          <option value="">All areas</option>
          {areas.map((a) => <option key={a} value={a}>{areaName(a)}</option>)}
        </select>
        <div className="relative min-w-52 flex-1">
          <Search size={14} className="absolute left-3 top-3 text-ink-500" />
          <input aria-label="Search the log" className={`${inputCls} pl-9`} placeholder="Search the log" value={q} onChange={(e) => setQ(e.target.value)} />
        </div>
      </div>
      <Card>
        {shown.length === 0 ? (
          <Empty title={all.length === 0 ? "Nothing logged yet" : "No entries match"}>
            {all.length === 0 ? "Entries appear here as StreamForge imports recordings, processes them and posts." : "Try another filter or clear the search."}
          </Empty>
        ) : (
          <ul className="divide-y divide-ink-800">{shown.map((a) => <ActivityRow key={a.id} a={a} />)}</ul>
        )}
      </Card>
    </Page>
  );
}
