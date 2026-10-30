import { useEffect, useState } from "react";
import {
  BarChart3,
  CalendarDays,
  Cable,
  Clapperboard,
  Film,
  Image,
  LayoutDashboard,
  ListChecks,
  Radio,
  Settings as SettingsIcon,
  TrendingUp,
  UploadCloud,
  Users,
} from "lucide-react";
import Dashboard from "./pages/Dashboard";
import Library from "./pages/Library";
import LiveMonitor from "./pages/LiveMonitor";
import Connections from "./pages/Connections";
import SettingsPage from "./pages/Settings";
import ClipStudio from "./pages/ClipStudio";
import Thumbnails from "./pages/Thumbnails";
import Trends from "./pages/Trends";
import Competitors from "./pages/Competitors";
import Calendar from "./pages/Calendar";
import UploadQueue from "./pages/UploadQueue";
import Analytics from "./pages/Analytics";
import Setup from "./pages/Setup";
import { api } from "./lib/api";

export type PageId =
  | "dashboard" | "live" | "library" | "clips" | "thumbnails" | "trends"
  | "competitors" | "calendar" | "uploads" | "analytics" | "connections" | "settings" | "setup";

const NAV: { group: string; items: { id: PageId; label: string; icon: typeof Film }[] }[] = [
  {
    group: "Overview",
    items: [
      { id: "dashboard", label: "Dashboard", icon: LayoutDashboard },
      { id: "live", label: "Live Stream Monitor", icon: Radio },
      { id: "library", label: "Video Library", icon: Film },
    ],
  },
  {
    group: "Create",
    items: [
      { id: "clips", label: "AI Clip Studio", icon: Clapperboard },
      { id: "thumbnails", label: "Thumbnail Studio", icon: Image },
    ],
  },
  {
    group: "Research",
    items: [
      { id: "trends", label: "Trend Research", icon: TrendingUp },
      { id: "competitors", label: "Competitor Watchlist", icon: Users },
    ],
  },
  {
    group: "Publish",
    items: [
      { id: "calendar", label: "Publishing Calendar", icon: CalendarDays },
      { id: "uploads", label: "Upload Queue", icon: UploadCloud },
      { id: "analytics", label: "Analytics", icon: BarChart3 },
    ],
  },
  {
    group: "Setup",
    items: [
      { id: "setup", label: "Setup guide", icon: ListChecks },
      { id: "connections", label: "Account Connections", icon: Cable },
      { id: "settings", label: "Settings", icon: SettingsIcon },
    ],
  },
];

export default function App() {
  const [page, setPage] = useState<PageId>("dashboard");

  // First launch opens the setup wizard until it's finished or skipped.
  useEffect(() => {
    api.settings().then((s) => !s.onboarded && setPage("setup")).catch(() => {});
  }, []);

  return (
    <div className="flex h-full">
      <aside className="flex w-60 shrink-0 flex-col border-r border-ink-800 bg-ink-900">
        <div className="flex items-center gap-2.5 px-5 py-5">
          <div className="h-7 w-7 rounded-lg bg-gradient-to-br from-accent to-indigo-500" />
          <div className="text-[15px] font-semibold tracking-tight">StreamForge AI</div>
        </div>
        <nav className="flex-1 overflow-y-auto px-3 pb-4">
          {NAV.map((g) => (
            <div key={g.group} className="mt-4 first:mt-1">
              <div className="px-2 pb-1.5 text-[11px] font-medium uppercase tracking-wider text-ink-500">{g.group}</div>
              {g.items.map(({ id, label, icon: Icon }) => (
                <button
                  key={id}
                  onClick={() => setPage(id)}
                  className={`flex w-full items-center gap-2.5 rounded-lg px-2.5 py-1.5 text-[13.5px] transition-colors ${
                    page === id ? "bg-ink-800 text-ink-100" : "text-ink-300 hover:bg-ink-850 hover:text-ink-100"
                  }`}
                >
                  <Icon size={16} strokeWidth={1.75} />
                  {label}
                </button>
              ))}
            </div>
          ))}
        </nav>
        <div className="border-t border-ink-800 px-5 py-3 text-[11px] text-ink-500">v0.2.0</div>
      </aside>
      <main className="flex-1 overflow-y-auto">
        {page === "dashboard" && <Dashboard go={setPage} />}
        {page === "live" && <LiveMonitor go={setPage} />}
        {page === "library" && <Library go={setPage} />}
        {page === "connections" && <Connections />}
        {page === "settings" && <SettingsPage />}
        {page === "setup" && <Setup go={setPage} />}
        {page === "clips" && <ClipStudio go={setPage} />}
        {page === "thumbnails" && <Thumbnails />}
        {page === "trends" && <Trends />}
        {page === "competitors" && <Competitors />}
        {page === "calendar" && <Calendar go={setPage} />}
        {page === "uploads" && <UploadQueue />}
        {page === "analytics" && <Analytics />}
      </main>
    </div>
  );
}
