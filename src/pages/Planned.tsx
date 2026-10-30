import { Badge, Card, Page } from "../components/ui";

// Pages whose engines aren't built yet. They say so plainly instead of showing
// mock data or buttons that do nothing (see ROADMAP.md).
const PLANNED: Record<string, { title: string; phase: number; what: string; points: string[] }> = {
  clips: {
    title: "AI Clip Studio",
    phase: 3,
    what: "Finds the best moments in each recording and turns them into 60–120 s vertical clips with animated subtitles.",
    points: [
      "Whisper transcription on your GPU (faster-whisper), Spanish and English detected automatically.",
      "Candidates come from audio energy, laughter and excitement, scene changes and the transcript.",
      "An LLM reads each candidate's transcript and scores hook, entertainment, emotion, context, replay and shareability.",
      "Clips are at least 60 s and extended with real surrounding context, never padded.",
      "Approve, reject, edit or regenerate each clip.",
    ],
  },
  thumbnails: {
    title: "Thumbnail Studio",
    phase: 4,
    what: "Three ranked 1280×720 thumbnail concepts per full video, plus vertical covers for Shorts and Reels.",
    points: ["Frame picking with face and expression detection", "Background removal and layered composition", "Typography checked for mobile readability"],
  },
  trends: {
    title: "Trend Research",
    phase: 4,
    what: "A daily report on formats, hooks, thumbnail styles, topics and hashtags in your niche.",
    points: ["Official APIs and public data only; no scraping behind logins or CAPTCHAs", "Other creators' videos are never downloaded or reposted"],
  },
  competitors: {
    title: "Competitor Watchlist",
    phase: 4,
    what: "Track public posts, views and posting frequency for creators you choose.",
    points: ["YouTube Data API for channel stats", "Other platforms only where an official API allows it"],
  },
  calendar: {
    title: "Publishing Calendar",
    phase: 6,
    what: "Scheduled posts across all platforms in one calendar, with drag-and-drop rescheduling.",
    points: [],
  },
  uploads: {
    title: "Upload Queue",
    phase: 5,
    what: "Every post waiting for review, scheduled, uploading or published, with retries and duplicate protection.",
    points: ["Review-before-publishing is on until you turn on full automation"],
  },
  analytics: {
    title: "Analytics",
    phase: 6,
    what: "Real platform analytics for your own posts, plus the feedback loop that tunes clip selection.",
    points: ["Only numbers returned by the platforms' APIs are shown; nothing is estimated or simulated"],
  },
};

export default function Planned({ page }: { page: string }) {
  const p = PLANNED[page];
  return (
    <Page title={p.title}>
      <Card title="Not built yet" right={<Badge>Phase {p.phase}</Badge>}>
        <p className="text-sm text-ink-100">{p.what}</p>
        {p.points.length > 0 && (
          <ul className="mt-3 list-disc space-y-1.5 pl-5 text-sm text-ink-300">
            {p.points.map((x) => (
              <li key={x}>{x}</li>
            ))}
          </ul>
        )}
      </Card>
    </Page>
  );
}
