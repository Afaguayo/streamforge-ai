import type { ButtonHTMLAttributes, ReactNode } from "react";

export function Page({ title, subtitle, actions, children }: { title: string; subtitle?: string; actions?: ReactNode; children: ReactNode }) {
  return (
    <div className="mx-auto max-w-6xl px-8 py-7">
      <header className="mb-6 flex items-end justify-between gap-6">
        <div>
          <h1 className="text-[22px] font-semibold tracking-tight">{title}</h1>
          {subtitle && <p className="mt-1 text-sm text-ink-300">{subtitle}</p>}
        </div>
        {actions && <div className="flex shrink-0 gap-2">{actions}</div>}
      </header>
      {children}
    </div>
  );
}

export function Card({ title, right, children, className = "" }: { title?: string; right?: ReactNode; children: ReactNode; className?: string }) {
  return (
    <section className={`rounded-xl border border-ink-800 bg-ink-900 ${className}`}>
      {title && (
        <div className="flex items-center justify-between border-b border-ink-800 px-5 py-3">
          <h2 className="text-[13px] font-medium uppercase tracking-wider text-ink-300">{title}</h2>
          {right}
        </div>
      )}
      <div className="p-5">{children}</div>
    </section>
  );
}

type Variant = "primary" | "ghost" | "danger";
const variants: Record<Variant, string> = {
  primary: "bg-ink-100 text-ink-950 hover:bg-white",
  ghost: "border border-ink-700 text-ink-100 hover:bg-ink-800",
  danger: "border border-red-900/60 text-red-300 hover:bg-red-950/40",
};

export function Button({ variant = "ghost", className = "", ...rest }: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: Variant }) {
  return (
    <button
      className={`inline-flex items-center gap-2 rounded-lg px-3.5 py-2 text-sm font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-40 ${variants[variant]} ${className}`}
      {...rest}
    />
  );
}

const tones = {
  neutral: "bg-ink-800 text-ink-300",
  good: "bg-emerald-950/60 text-emerald-300",
  warn: "bg-amber-950/60 text-amber-300",
  bad: "bg-red-950/60 text-red-300",
  live: "bg-kick/15 text-kick",
  info: "bg-sky-950/60 text-sky-300",
};

export function Badge({ tone = "neutral", children }: { tone?: keyof typeof tones; children: ReactNode }) {
  return <span className={`inline-flex items-center gap-1.5 rounded-md px-2 py-0.5 text-xs font-medium ${tones[tone]}`}>{children}</span>;
}

export function Stat({ label, value, hint }: { label: string; value: ReactNode; hint?: ReactNode }) {
  return (
    <div className="rounded-xl border border-ink-800 bg-ink-900 px-5 py-4">
      <div className="text-xs uppercase tracking-wider text-ink-500">{label}</div>
      <div className="mt-1.5 text-2xl font-semibold tabular-nums">{value}</div>
      {hint && <div className="mt-1 text-xs text-ink-500">{hint}</div>}
    </div>
  );
}

export function Empty({ children }: { children: ReactNode }) {
  return <div className="rounded-lg border border-dashed border-ink-700 px-5 py-8 text-center text-sm text-ink-500">{children}</div>;
}

export function Field({ label, hint, children }: { label: string; hint?: ReactNode; children: ReactNode }) {
  return (
    <label className="block">
      <span className="text-sm font-medium">{label}</span>
      {hint && <span className="mt-0.5 block text-xs text-ink-500">{hint}</span>}
      <div className="mt-2">{children}</div>
    </label>
  );
}

export const inputCls =
  "w-full rounded-lg border border-ink-700 bg-ink-950 px-3 py-2 text-sm outline-none placeholder:text-ink-500 focus:border-ink-500";

export function Toggle({ checked, onChange, label, hint }: { checked: boolean; onChange: (v: boolean) => void; label: string; hint?: string }) {
  return (
    <button type="button" onClick={() => onChange(!checked)} className="flex w-full items-start justify-between gap-6 text-left">
      <span>
        <span className="block text-sm font-medium">{label}</span>
        {hint && <span className="mt-0.5 block text-xs text-ink-500">{hint}</span>}
      </span>
      <span className={`mt-0.5 flex h-5 w-9 shrink-0 items-center rounded-full p-0.5 transition-colors ${checked ? "bg-emerald-500" : "bg-ink-700"}`}>
        <span className={`h-4 w-4 rounded-full bg-white transition-transform ${checked ? "translate-x-4" : ""}`} />
      </span>
    </button>
  );
}

export function Notice({ tone = "info", children }: { tone?: "info" | "warn" | "bad" | "good"; children: ReactNode }) {
  const c = {
    info: "border-sky-900/50 bg-sky-950/30 text-sky-200",
    warn: "border-amber-900/50 bg-amber-950/30 text-amber-200",
    bad: "border-red-900/50 bg-red-950/30 text-red-200",
    good: "border-emerald-900/50 bg-emerald-950/30 text-emerald-200",
  }[tone];
  return <div className={`rounded-lg border px-4 py-3 text-sm ${c}`}>{children}</div>;
}
