import { useEffect, useRef, useState } from "react";
import { fileUrl } from "../lib/api";
import { Button, inputCls } from "./ui";

export type Layout = "blur" | "facecam";
export type Rect = [number, number, number, number];

/** Where the webcam sits in the owner's OBS scene (same default as the Rust side). */
export const FACECAM_DEFAULT: Rect = [0.0, 0.34, 0.29, 0.28];

// The 9:16 clip is 1080x1920. These bands mirror render.rs so the diagrams match the real output.
const HOOK_BAND = 220;
const CAM_H = 540;
const GAME_H = 856;
const BLUR_GAME_TOP = (1920 - 608) / 2 - 110;

const MIN = 0.05;
const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));
const r3 = (v: number) => Math.round(v * 1000) / 1000;

/** The webcam box the same way the renderer keeps it: inside the frame and never empty. */
function safeRect([x, y, w, h]: Rect): Rect {
  const cx = clamp(x, 0, 0.95);
  const cy = clamp(y, 0, 0.95);
  return [cx, cy, clamp(w, MIN, 1 - cx), clamp(h, MIN, 1 - cy)];
}

/** The slice of the recording's width the gameplay band shows (render.rs `game_x`), as fractions. */
function gameWindow(rect: Rect): [number, number] {
  const [x, , w] = safeRect(rect);
  const span = Math.min(1, (1080 / GAME_H) * (9 / 16));
  const centred = (1 - span) / 2;
  let left = centred;
  if (x + w <= 0.5) left = Math.max(centred, Math.min(x + w, 1 - span));
  else if (x >= 0.5) left = Math.min(centred, Math.max(x - span, 0));
  return [left, span];
}

/** What the webcam band actually shows: it fills a 2:1 strip, so a box of another shape loses its edges. */
function camVisible(rect: Rect): Rect {
  const [x, y, w, h] = safeRect(rect);
  const ratio = (w * 16) / (h * 9); // box width / height in pixels
  const target = 1080 / CAM_H;
  if (ratio > target) {
    const nw = w * (target / ratio);
    return [x + (w - nw) / 2, y, nw, h];
  }
  const nh = h * (ratio / target);
  return [x, y + (h - nh) / 2, w, nh];
}

const LEGEND = {
  hook: ["#fbbf24", "Opening hook text"],
  cam: ["#34d399", "Your webcam"],
  game: ["#38bdf8", "Gameplay"],
  caps: ["#e6e9ef", "Captions"],
} as const;

/** A small drawing of the finished 9:16 clip: where the hook, webcam, gameplay and captions go. */
export function ClipDiagram({ layout, width = 84 }: { layout: Layout; width?: number }) {
  const bar = (y: number, color: string, w = 760) => <rect x={(1080 - w) / 2} y={y} width={w} height={70} rx={20} fill={color} />;
  return (
    <svg viewBox="0 0 1080 1920" width={width} height={(width * 1920) / 1080} role="img" aria-label={layout === "facecam" ? "Clip with webcam on top and gameplay below" : "Clip with gameplay centred on a blurred background"} className="shrink-0 rounded-md bg-ink-700">
      {layout === "facecam" ? (
        <>
          {bar(60, LEGEND.hook[0])}
          <rect x={0} y={HOOK_BAND} width={1080} height={CAM_H} fill={LEGEND.cam[0]} opacity={0.85} />
          <rect x={0} y={HOOK_BAND + CAM_H} width={1080} height={GAME_H} fill={LEGEND.game[0]} opacity={0.85} />
          {bar(HOOK_BAND + CAM_H + GAME_H + 50, LEGEND.caps[0], 880)}
          {bar(HOOK_BAND + CAM_H + GAME_H + 150, LEGEND.caps[0], 560)}
        </>
      ) : (
        <>
          {bar(250 - 20, LEGEND.hook[0])}
          <rect x={0} y={BLUR_GAME_TOP} width={1080} height={608} fill={LEGEND.game[0]} opacity={0.85} />
          {bar(BLUR_GAME_TOP + 608 + 50, LEGEND.caps[0], 880)}
          {bar(BLUR_GAME_TOP + 608 + 150, LEGEND.caps[0], 560)}
        </>
      )}
    </svg>
  );
}

function Legend({ layout }: { layout: Layout }) {
  const keys = layout === "facecam" ? (["hook", "cam", "game", "caps"] as const) : (["hook", "game", "caps"] as const);
  return (
    <ul className="space-y-1 text-[13px] text-ink-300">
      {keys.map((k) => (
        <li key={k} className="flex items-center gap-2"><span className="h-2.5 w-2.5 rounded-sm" style={{ background: LEGEND[k][0] }} />{LEGEND[k][1]}</li>
      ))}
      {layout === "blur" && <li className="flex items-center gap-2"><span className="h-2.5 w-2.5 rounded-sm bg-ink-500" />Blurred copy of the stream</li>}
    </ul>
  );
}

const OPTIONS: { id: Layout; title: string; text: string }[] = [
  { id: "blur", title: "Gameplay on a blurred background", text: "The whole stream, centred, over a blurred copy of itself. Works with any scene. Your webcam stays small inside the gameplay." },
  { id: "facecam", title: "Webcam on top, gameplay below", text: "Your face gets its own band above the gameplay, like most Shorts. Needs the webcam box set below." },
];

/** A 16:9 frame with a draggable, resizable webcam box. Arrow keys nudge it, Shift+arrows resize it. */
function CamBox({ rect, onChange, src }: { rect: Rect; onChange: (r: Rect) => void; src: string | null }) {
  const box = useRef<HTMLDivElement>(null);
  const drag = useRef<{ mode: string; x: number; y: number; rect: Rect } | null>(null);
  const [broken, setBroken] = useState(false);
  useEffect(() => setBroken(false), [src]);

  const down = (mode: string) => (e: React.PointerEvent) => {
    e.preventDefault();
    e.stopPropagation();
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    drag.current = { mode, x: e.clientX, y: e.clientY, rect: [...rect] as Rect };
  };
  const move = (e: React.PointerEvent) => {
    const d = drag.current;
    if (!d || !box.current) return;
    const b = box.current.getBoundingClientRect();
    const dx = (e.clientX - d.x) / b.width;
    const dy = (e.clientY - d.y) / b.height;
    let [x, y, w, h] = d.rect;
    if (d.mode === "move") {
      x = clamp(x + dx, 0, 1 - w);
      y = clamp(y + dy, 0, 1 - h);
    } else {
      let x2 = x + w;
      let y2 = y + h;
      if (d.mode.includes("w")) x = clamp(x + dx, 0, x2 - MIN);
      else x2 = clamp(x2 + dx, x + MIN, 1);
      if (d.mode.includes("n")) y = clamp(y + dy, 0, y2 - MIN);
      else y2 = clamp(y2 + dy, y + MIN, 1);
      w = x2 - x;
      h = y2 - y;
    }
    onChange([r3(x), r3(y), r3(w), r3(h)]);
  };
  const up = () => (drag.current = null);
  const key = (e: React.KeyboardEvent) => {
    const step = e.key === "ArrowLeft" ? [-1, 0] : e.key === "ArrowRight" ? [1, 0] : e.key === "ArrowUp" ? [0, -1] : e.key === "ArrowDown" ? [0, 1] : null;
    if (!step) return;
    e.preventDefault();
    const [x, y, w, h] = safeRect(rect);
    const s = 0.01;
    if (e.shiftKey) onChange([x, y, r3(clamp(w + step[0] * s, MIN, 1 - x)), r3(clamp(h + step[1] * s, MIN, 1 - y))]);
    else onChange([r3(clamp(x + step[0] * s, 0, 1 - w)), r3(clamp(y + step[1] * s, 0, 1 - h)), w, h]);
  };

  const [x, y, w, h] = safeRect(rect);
  const [gl, gs] = gameWindow(rect);
  const [vx, vy, vw, vh] = camVisible(rect);
  const pct = (v: number) => `${v * 100}%`;
  const corners = [["nw", "left-0 top-0 -translate-x-1/2 -translate-y-1/2 cursor-nwse-resize"], ["ne", "right-0 top-0 translate-x-1/2 -translate-y-1/2 cursor-nesw-resize"], ["sw", "left-0 bottom-0 -translate-x-1/2 translate-y-1/2 cursor-nesw-resize"], ["se", "right-0 bottom-0 translate-x-1/2 translate-y-1/2 cursor-nwse-resize"]];
  return (
    <div ref={box} className="relative aspect-video w-full select-none overflow-hidden rounded-md bg-ink-800 touch-none">
      {src && !broken ? (
        <video
          src={src}
          muted
          preload="metadata"
          className="pointer-events-none absolute inset-0 h-full w-full object-cover"
          onLoadedMetadata={(e) => { const v = e.currentTarget; v.currentTime = Math.min(Math.max(v.duration * 0.15, 1), 600); }}
          onError={() => setBroken(true)}
        />
      ) : (
        <div className="absolute inset-0 flex items-center justify-center text-[13px] text-ink-400">{src ? "This recording cannot be previewed here" : "Plain 16:9 frame. Import a recording to see a real frame here."}</div>
      )}
      {/* Gameplay band: the part of the frame that ends up below the webcam. */}
      <div className="pointer-events-none absolute inset-y-0 border-x-2 border-dashed border-sky-400/80 bg-sky-400/10" style={{ left: pct(gl), width: pct(gs) }}>
        <span className="absolute bottom-1 left-1 rounded bg-black/60 px-1.5 py-0.5 text-xs text-sky-300">Gameplay shown in the clip</span>
      </div>
      {/* What the webcam band really shows from the box. */}
      <div className="pointer-events-none absolute border border-dashed border-white/70" style={{ left: pct(vx), top: pct(vy), width: pct(vw), height: pct(vh) }} />
      <div
        role="group"
        tabIndex={0}
        aria-label="Webcam box. Arrow keys move it, Shift plus arrow keys resize it."
        onKeyDown={key}
        onPointerDown={down("move")}
        onPointerMove={move}
        onPointerUp={up}
        className="absolute cursor-move border-2 border-emerald-400 bg-emerald-400/15"
        style={{ left: pct(x), top: pct(y), width: pct(w), height: pct(h) }}
      >
        <span className="pointer-events-none absolute left-1 top-1 rounded bg-black/60 px-1.5 py-0.5 text-xs text-emerald-300">Webcam</span>
        {corners.map(([m, cls]) => (
          <span key={m} onPointerDown={down(m)} onPointerMove={move} onPointerUp={up} className={`absolute h-3.5 w-3.5 rounded-sm border-2 border-ink-950 bg-emerald-400 ${cls}`} />
        ))}
      </div>
    </div>
  );
}

/** Picks the clip layout with a drawing of each result, and places the webcam box visually. */
export function LayoutPicker({ layout, rect, onLayout, onRect, recording }: { layout: Layout; rect: Rect; onLayout: (l: Layout) => void; onRect: (r: Rect) => void; recording: string | null }) {
  const setNum = (i: number, v: number) => {
    const r = [...rect] as Rect;
    r[i] = clamp(v / 100, 0, 1);
    onRect(r);
  };
  const [x, , w, h] = safeRect(rect);
  const camRatio = (w * 16) / (h * 9);
  return (
    <div className="space-y-5">
      <div role="radiogroup" aria-label="Clip layout" className="grid gap-3 lg:grid-cols-2">
        {OPTIONS.map((o) => (
          <button key={o.id} type="button" role="radio" aria-checked={layout === o.id} onClick={() => onLayout(o.id)}
            className={`flex gap-4 rounded-lg bg-ink-950 p-4 text-left transition-colors ${layout === o.id ? "ring-2 ring-kick" : "ring-1 ring-ink-700 hover:ring-ink-500"}`}>
            <ClipDiagram layout={o.id} />
            <span className="min-w-0 space-y-2">
              <span className="block text-sm font-medium">{o.title}{layout === o.id && <span className="ml-2 text-xs font-normal text-kick">Selected</span>}</span>
              <span className="block text-[13px] leading-snug text-ink-300">{o.text}</span>
              <Legend layout={o.id} />
            </span>
          </button>
        ))}
      </div>

      {layout === "facecam" && (
        <div>
          <div className="text-sm font-medium">Where your webcam is in the recording</div>
          <p className="mt-0.5 text-[13px] leading-snug text-ink-400">Drag the green box over your webcam and pull its corners to fit it. The blue strip is the part of the stream that becomes the gameplay under your face. The dashed white line is what the webcam band really shows.</p>
          <div className="mt-3 grid gap-4 lg:grid-cols-[minmax(0,1fr)_auto]">
            <CamBox rect={rect} onChange={onRect} src={recording ? fileUrl(recording) : null} />
            <div className="flex items-start gap-4">
              <div className="space-y-1.5">
                <div className="text-[13px] text-ink-300">Clip result</div>
                <ClipDiagram layout="facecam" width={96} />
              </div>
              <div className="w-36 space-y-2">
                {(["Left", "Top", "Width", "Height"] as const).map((label, i) => (
                  <label key={label} className="flex items-center justify-between gap-2 text-[13px] text-ink-300">
                    {label} %
                    <input type="number" min={0} max={100} step={0.5} className={`${inputCls} !w-20`} value={Math.round(rect[i] * 1000) / 10} onChange={(e) => setNum(i, Number(e.target.value))} />
                  </label>
                ))}
                <Button size="sm" className="w-full" onClick={() => onRect([...FACECAM_DEFAULT] as Rect)}>Reset to default</Button>
              </div>
            </div>
          </div>
          {Math.abs(camRatio - 2) > 0.45 && (
            <p className="mt-2 text-[13px] text-amber-300">This box is {camRatio > 2 ? "wider" : "taller"} than the 2:1 band your webcam fills in the clip, so some of its {camRatio > 2 ? "left and right" : "top and bottom"} edges are trimmed (the dashed white line).</p>
          )}
          {x + w > 0.5 && x < 0.5 && <p className="mt-2 text-[13px] text-amber-300">A webcam across the middle of the frame cannot be left out of the gameplay band, so part of it shows twice.</p>}
        </div>
      )}
    </div>
  );
}
