import { useEffect, useState } from "react";
import {
  BarChart3,
  CalendarDays,
  Cable,
  Clapperboard,
  Film,
  Gauge,
  Image,
  Scissors,
  LayoutDashboard,
  ListChecks,
  Radio,
  ScrollText,
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
import ProgressPage from "./pages/Progress";
import ActivityPage from "./pages/Activity";
import ClipPicker from "./pages/ClipPicker";
import { ProgressContext, ProgressDock, useProgress } from "./components/Progress";
import { ConfirmProvider } from "./components/ui";
import { api } from "./lib/api";
import { usePoll } from "./lib/usePoll";

export type PageId =
  | "dashboard" | "progress" | "activity" | "live" | "library" | "clips" | "picker" | "thumbnails" | "trends"
  | "competitors" | "calendar" | "uploads" | "analytics" | "connections" | "settings" | "setup";

type NavItem = { id: PageId; label: string; icon: typeof Film };

// Grouped in the order the work happens: streams come in, clips are made, posts go out.
const NAV: { group: string; items: NavItem[] }[] = [
  {
    group: "Today",
    items: [
      { id: "dashboard", label: "Dashboard", icon: LayoutDashboard },
      { id: "progress", label: "What's running", icon: Gauge },
      { id: "activity", label: "Activity log", icon: ScrollText },
    ],
  },
  {
    group: "Streams",
    items: [
      { id: "live", label: "Live monitor", icon: Radio },
      { id: "library", label: "Recordings", icon: Film },
    ],
  },
  {
    group: "Clips",
    items: [
      { id: "clips", label: "AI clips", icon: Clapperboard },
      { id: "picker", label: "Cut your own", icon: Scissors },
      { id: "thumbnails", label: "Thumbnails", icon: Image },
    ],
  },
  {
    group: "Publish",
    items: [
      { id: "uploads", label: "Upload queue", icon: UploadCloud },
      { id: "calendar", label: "Calendar", icon: CalendarDays },
    ],
  },
  {
    group: "Insights",
    items: [
      { id: "analytics", label: "Analytics", icon: BarChart3 },
      { id: "trends", label: "Trends", icon: TrendingUp },
      { id: "competitors", label: "Competitors", icon: Users },
    ],
  },
  {
    group: "Settings",
    items: [
      { id: "setup", label: "Setup guide", icon: ListChecks },
      { id: "connections", label: "Accounts", icon: Cable },
      { id: "settings", label: "Settings", icon: SettingsIcon },
    ],
  },
];

export default function App() {
  const [page, setPage] = useState<PageId>("dashboard");
  const progress = useProgress();
  const { data: posts } = usePoll(api.posts, 10000);
  const { data: clips } = usePoll(api.clips, 10000);

  // First launch opens the setup wizard until it's finished or skipped.
  useEffect(() => {
    api.settings().then((s) => !s.onboarded && setPage("setup")).catch(() => {});
  }, []);

  // Numbers beside a page name mean "this needs you".
  const badge: Partial<Record<PageId, { n: number; bad?: boolean }>> = {
    progress: { n: progress.failed.length, bad: true },
    clips: { n: (clips ?? []).filter((c) => c.status === "suggested").length },
    uploads: { n: (posts ?? []).filter((p) => p.status === "draft" || p.status === "failed").length },
  };

  return (
    <ConfirmProvider>
      <ProgressContext.Provider value={progress}>
        <div className="flex h-full">
          <aside className="flex w-56 shrink-0 flex-col bg-ink-900">
            <div className="flex items-center gap-2.5 px-5 py-5">
              <div className="h-7 w-7 rounded-lg bg-kick" />
              <div className="text-[15px] font-semibold tracking-tight">StreamForge AI</div>
            </div>
            <nav aria-label="Main" className="flex-1 overflow-y-auto px-3 pb-4">
              {NAV.map((g) => (
                <div key={g.group} className="mt-4 first:mt-1">
                  <div className="px-2.5 pb-1 text-xs font-medium text-ink-400">{g.group}</div>
                  {g.items.map(({ id, label, icon: Icon }) => {
                    const b = badge[id];
                    return (
                      <button
                        key={id}
                        onClick={() => setPage(id)}
                        aria-current={page === id ? "page" : undefined}
                        className={`flex w-full items-center gap-2.5 rounded-md px-2.5 py-1.5 text-sm transition-colors ${
                          page === id ? "bg-ink-700 text-ink-100" : "text-ink-300 hover:bg-ink-800 hover:text-ink-100"
                        }`}
                      >
                        <Icon size={16} strokeWidth={1.75} />
                        <span className="flex-1 truncate text-left">{label}</span>
                        {b && b.n > 0 && (
                          <span className={`rounded-full px-1.5 text-xs font-medium tabular-nums ${b.bad ? "bg-red-900/70 text-red-200" : "bg-ink-700 text-ink-100"}`}>{b.n}</span>
                        )}
                      </button>
                    );
                  })}
                </div>
              ))}
            </nav>
            <ProgressDock p={progress} go={setPage} />
            <div className="px-5 py-3 text-xs text-ink-500">v0.3.1</div>
          </aside>
          <main className="min-w-0 flex-1 overflow-y-auto">
            {page === "dashboard" && <Dashboard go={setPage} />}
            {page === "progress" && <ProgressPage p={progress} />}
            {page === "activity" && <ActivityPage />}
            {page === "live" && <LiveMonitor go={setPage} />}
            {page === "library" && <Library go={setPage} />}
            {page === "connections" && <Connections />}
            {page === "settings" && <SettingsPage />}
            {page === "setup" && <Setup go={setPage} />}
            {page === "clips" && <ClipStudio go={setPage} />}
            {page === "picker" && <ClipPicker go={setPage} />}
            {page === "thumbnails" && <Thumbnails />}
            {page === "trends" && <Trends />}
            {page === "competitors" && <Competitors />}
            {page === "calendar" && <Calendar go={setPage} />}
            {page === "uploads" && <UploadQueue go={setPage} />}
            {page === "analytics" && <Analytics />}
          </main>
        </div>
      </ProgressContext.Provider>
    </ConfirmProvider>
  );
}
