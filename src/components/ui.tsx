import { AlertTriangle, Check, CircleDot } from "lucide-react";
import { createContext, useCallback, useContext, useEffect, useRef, useState, type ButtonHTMLAttributes, type ReactNode } from "react";

export function Page({ title, subtitle, actions, children }: { title: string; subtitle?: string; actions?: ReactNode; children: ReactNode }) {
  return (
    <div className="mx-auto max-w-6xl px-6 py-6">
      <header className="mb-5 flex flex-wrap items-end justify-between gap-x-6 gap-y-3">
        <div className="min-w-0 max-w-2xl">
          <h1 className="text-2xl font-semibold tracking-tight">{title}</h1>
          {subtitle && <p className="mt-1 text-sm leading-relaxed text-ink-300">{subtitle}</p>}
        </div>
        {actions && <div className="flex shrink-0 flex-wrap gap-2">{actions}</div>}
      </header>
      {children}
    </div>
  );
}

export function Card({ title, right, children, className = "" }: { title?: string; right?: ReactNode; children: ReactNode; className?: string }) {
  return (
    <section className={`rounded-lg bg-ink-900 ${className}`}>
      {title && (
        <div className="flex items-center justify-between gap-3 px-5 pt-4">
          <h2 className="text-[15px] font-semibold text-ink-100">{title}</h2>
          {right}
        </div>
      )}
      <div className="p-5">{children}</div>
    </section>
  );
}

type Variant = "primary" | "ghost" | "danger";
const variants: Record<Variant, string> = {
  primary: "bg-kick text-ink-950 hover:bg-[#7bff4d]",
  ghost: "bg-ink-800 text-ink-100 hover:bg-ink-700",
  danger: "bg-red-950/60 text-red-200 hover:bg-red-900/60",
};

export function Button({ variant = "ghost", size = "md", className = "", ...rest }: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: Variant; size?: "sm" | "md" }) {
  return (
    <button
      className={`inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-40 ${size === "sm" ? "px-2.5 py-1.5 text-[13px]" : "px-3.5 py-2 text-sm"} ${variants[variant]} ${className}`}
      {...rest}
    />
  );
}

/** A small icon-only button. `label` is the tooltip and the screen-reader name. */
export function IconButton({ label, children, className = "", ...rest }: ButtonHTMLAttributes<HTMLButtonElement> & { label: string }) {
  return (
    <button title={label} aria-label={label} className={`inline-flex h-8 w-8 items-center justify-center rounded-md text-ink-300 transition-colors hover:bg-ink-800 hover:text-white disabled:opacity-40 ${className}`} {...rest}>
      {children}
    </button>
  );
}

export const tones = {
  neutral: "bg-ink-800 text-ink-300",
  good: "bg-emerald-950/70 text-emerald-300",
  warn: "bg-amber-950/70 text-amber-300",
  bad: "bg-red-950/70 text-red-300",
  live: "bg-kick/15 text-kick",
  info: "bg-sky-950/70 text-sky-300",
};
export type Tone = keyof typeof tones;

export function Badge({ tone = "neutral", children }: { tone?: Tone; children: ReactNode }) {
  return <span className={`inline-flex items-center gap-1.5 whitespace-nowrap rounded px-2 py-0.5 text-xs font-medium ${tones[tone]}`}>{children}</span>;
}

export function Stat({ label, value, hint, onClick }: { label: string; value: ReactNode; hint?: ReactNode; onClick?: () => void }) {
  const body = (
    <>
      <div className="text-[13px] text-ink-300">{label}</div>
      <div className="mt-1 text-2xl font-semibold tabular-nums">{value}</div>
      {hint && <div className="mt-1 text-xs text-ink-400">{hint}</div>}
    </>
  );
  const cls = "rounded-lg bg-ink-900 px-5 py-4 text-left";
  return onClick ? <button onClick={onClick} className={`${cls} block w-full hover:bg-ink-850`}>{body}</button> : <div className={cls}>{body}</div>;
}

/** An empty list says what to do next: `children` is the explanation, `action` the next step. */
export function Empty({ children, title, action }: { children: ReactNode; title?: string; action?: ReactNode }) {
  return (
    <div className="rounded-lg bg-ink-950/60 px-5 py-8 text-center">
      {title && <div className="text-sm font-medium text-ink-100">{title}</div>}
      <div className={`mx-auto max-w-md text-sm leading-relaxed text-ink-300 ${title ? "mt-1" : ""}`}>{children}</div>
      {action && <div className="mt-4 flex justify-center gap-2">{action}</div>}
    </div>
  );
}

export function Field({ label, hint, children }: { label: string; hint?: ReactNode; children: ReactNode }) {
  return (
    <label className="block">
      <span className="text-sm font-medium">{label}</span>
      {hint && <span className="mt-0.5 block text-[13px] leading-snug text-ink-400">{hint}</span>}
      <div className="mt-2">{children}</div>
    </label>
  );
}

export const inputCls =
  "field-input rounded-md bg-ink-950 px-3 py-2 text-sm ring-1 ring-ink-700 placeholder:text-ink-500 hover:ring-ink-500 focus:ring-2 focus:ring-kick";

export function Toggle({ checked, onChange, label, hint }: { checked: boolean; onChange: (v: boolean) => void; label: string; hint?: string }) {
  return (
    <button type="button" role="switch" aria-checked={checked} onClick={() => onChange(!checked)} className="flex w-full items-start justify-between gap-6 text-left">
      <span>
        <span className="block text-sm font-medium">{label}</span>
        {hint && <span className="mt-0.5 block text-[13px] leading-snug text-ink-400">{hint}</span>}
      </span>
      <span className={`mt-0.5 flex h-5 w-9 shrink-0 items-center rounded-full p-0.5 transition-colors ${checked ? "bg-emerald-500" : "bg-ink-700"}`}>
        <span className={`h-4 w-4 rounded-full bg-white transition-transform ${checked ? "translate-x-4" : ""}`} />
      </span>
    </button>
  );
}

export function Notice({ tone = "info", children }: { tone?: "info" | "warn" | "bad" | "good"; children: ReactNode }) {
  const c = {
    info: "bg-sky-950/40 text-sky-100",
    warn: "bg-amber-950/40 text-amber-100",
    bad: "bg-red-950/40 text-red-100",
    good: "bg-emerald-950/40 text-emerald-100",
  }[tone];
  return <div role={tone === "bad" ? "alert" : undefined} className={`rounded-md px-4 py-3 text-sm leading-relaxed ${c}`}>{children}</div>;
}

export function ProgressBar({ value, tone = "info" }: { value: number; tone?: "info" | "good" | "bad" }) {
  const color = { info: "bg-sky-400", good: "bg-emerald-400", bad: "bg-red-400" }[tone];
  const pct = Math.round(Math.min(1, Math.max(0, value)) * 100);
  return (
    <div className="h-1.5 overflow-hidden rounded bg-ink-800" role="progressbar" aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100}>
      <div className={`h-full rounded ${color} transition-all duration-700`} style={{ width: `${pct}%` }} />
    </div>
  );
}

/** Switches between a few views. */
export function Tabs<T extends string>({ tabs, value, onChange }: { tabs: { id: T; label: string; count?: number }[]; value: T; onChange: (id: T) => void }) {
  return (
    <div role="tablist" className="flex w-fit flex-wrap gap-1 rounded-lg bg-ink-900 p-1 text-sm">
      {tabs.map((t) => (
        <button key={t.id} role="tab" aria-selected={value === t.id} onClick={() => onChange(t.id)}
          className={`rounded-md px-3 py-1.5 transition-colors ${value === t.id ? "bg-ink-700 text-ink-100" : "text-ink-300 hover:text-ink-100"}`}>
          {t.label}
          {t.count != null && <span className={`ml-1.5 tabular-nums ${value === t.id ? "text-ink-300" : "text-ink-500"}`}>{t.count}</span>}
        </button>
      ))}
    </div>
  );
}

/** Label and value pairs for the details of a stream, clip or post. */
export function Facts({ items }: { items: [string, ReactNode][] }) {
  const shown = items.filter(([, v]) => v != null && v !== false && v !== "");
  return (
    <dl className="grid grid-cols-[repeat(auto-fill,minmax(150px,1fr))] gap-x-5 gap-y-2.5 text-sm">
      {shown.map(([k, v]) => (
        <div key={k} className="min-w-0">
          <dt className="text-xs text-ink-400">{k}</dt>
          <dd className="mt-0.5 break-words text-ink-100">{v}</dd>
        </div>
      ))}
    </dl>
  );
}

/** Where something is in a pipeline: done steps, the current one, the rest. */
export function Stepper({ steps, current, failed = false, finished = false }: { steps: string[]; current: number; failed?: boolean; finished?: boolean }) {
  return (
    <ol className="flex flex-wrap items-center gap-x-1 gap-y-1 text-xs">
      {steps.map((s, i) => {
        const done = finished || i < current;
        const here = !finished && i === current;
        const color = here ? (failed ? "text-red-300" : "text-sky-300") : done ? "text-emerald-300" : "text-ink-500";
        return (
          <li key={s} className={`flex items-center gap-1 ${color}`} aria-current={here ? "step" : undefined}>
            {done ? <Check size={13} /> : here ? (failed ? <AlertTriangle size={13} /> : <CircleDot size={13} />) : <span className="inline-block h-[13px] w-[13px] scale-50 rounded-full bg-ink-700" />}
            <span className={here ? "font-medium" : ""}>{s}</span>
            {i < steps.length - 1 && <span className="mx-1 text-ink-700">›</span>}
          </li>
        );
      })}
    </ol>
  );
}

// ---- Confirmation for destructive actions ----

type ConfirmOpts = { title: string; body?: ReactNode; confirm?: string };
const ConfirmCtx = createContext<(o: ConfirmOpts) => Promise<boolean>>(() => Promise.resolve(true));

/** Ask before deleting or discarding: `if (await confirm({ title: "Delete this draft?" })) …`. */
export const useConfirm = () => useContext(ConfirmCtx);

export function ConfirmProvider({ children }: { children: ReactNode }) {
  const [ask, setAsk] = useState<(ConfirmOpts & { done: (ok: boolean) => void }) | null>(null);
  const confirm = useCallback((o: ConfirmOpts) => new Promise<boolean>((done) => setAsk({ ...o, done })), []);
  const cancel = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (!ask) return;
    cancel.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      ask.done(false);
      setAsk(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [ask]);
  const close = (ok: boolean) => {
    ask?.done(ok);
    setAsk(null);
  };
  return (
    <ConfirmCtx.Provider value={confirm}>
      {children}
      {ask && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-6" onClick={() => close(false)}>
          <div role="alertdialog" aria-modal="true" aria-label={ask.title} className="w-full max-w-md rounded-lg bg-ink-800 p-6 shadow-2xl" onClick={(e) => e.stopPropagation()}>
            <h2 className="text-base font-semibold">{ask.title}</h2>
            {ask.body && <div className="mt-2 text-sm leading-relaxed text-ink-300">{ask.body}</div>}
            <div className="mt-5 flex justify-end gap-2">
              <button ref={cancel} onClick={() => close(false)} className="rounded-md bg-ink-700 px-3.5 py-2 text-sm font-medium hover:bg-ink-500">Cancel</button>
              <button onClick={() => close(true)} className="rounded-md bg-red-600 px-3.5 py-2 text-sm font-medium text-white hover:bg-red-500">{ask.confirm ?? "Delete"}</button>
            </div>
          </div>
        </div>
      )}
    </ConfirmCtx.Provider>
  );
}
