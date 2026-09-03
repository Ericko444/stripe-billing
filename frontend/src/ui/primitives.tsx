import type { ButtonHTMLAttributes, ReactNode } from "react";

/**
 * The small set of presentational pieces the four panels share. Each one
 * exists because three or more call sites wanted it; nothing here is
 * speculative, and none of it knows anything about billing.
 *
 * Accessibility attributes that the tests and screen readers rely on
 * (`role="alert"`, `aria-busy`) are carried by these wrappers rather than
 * re-typed at every call site, which is most of why they exist.
 */

/** A titled panel. `action` sits opposite the title in the header. */
export function Card({
  title,
  subtitle,
  action,
  children,
}: {
  title: string;
  subtitle?: string;
  action?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="overflow-hidden rounded-2xl border border-slate-200 bg-white shadow-sm">
      <header className="flex items-start justify-between gap-4 border-b border-slate-100 px-6 py-4">
        <div>
          <h2 className="text-[13px] font-semibold uppercase tracking-[0.08em] text-slate-500">
            {title}
          </h2>
          {subtitle && <p className="mt-1 text-sm text-slate-400">{subtitle}</p>}
        </div>
        {action}
      </header>
      <div className="px-6 py-5">{children}</div>
    </section>
  );
}

const BUTTON_BASE =
  "inline-flex items-center justify-center gap-1.5 whitespace-nowrap rounded-lg px-3 py-1.5 " +
  "text-sm font-medium transition-colors focus-visible:outline-2 focus-visible:outline-offset-2 " +
  "disabled:cursor-not-allowed disabled:opacity-50";

const BUTTON_VARIANTS = {
  primary: "bg-indigo-600 text-white shadow-sm hover:bg-indigo-500 focus-visible:outline-indigo-600",
  secondary:
    "border border-slate-300 bg-white text-slate-700 shadow-sm hover:bg-slate-50 focus-visible:outline-slate-500",
  danger: "bg-red-600 text-white shadow-sm hover:bg-red-500 focus-visible:outline-red-600",
  ghost: "text-slate-600 hover:bg-slate-100 focus-visible:outline-slate-500",
} as const;

export function Button({
  variant = "secondary",
  className = "",
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: keyof typeof BUTTON_VARIANTS }) {
  return <button className={`${BUTTON_BASE} ${BUTTON_VARIANTS[variant]} ${className}`} {...props} />;
}

const BADGE_TONES = {
  green: "bg-emerald-50 text-emerald-700 ring-emerald-600/20",
  amber: "bg-amber-50 text-amber-800 ring-amber-600/20",
  slate: "bg-slate-100 text-slate-600 ring-slate-500/20",
  red: "bg-red-50 text-red-700 ring-red-600/20",
} as const;

export function Badge({
  tone = "slate",
  children,
}: {
  tone?: keyof typeof BADGE_TONES;
  children: ReactNode;
}) {
  return (
    <span
      className={`inline-flex items-center rounded-full px-2.5 py-0.5 text-xs font-medium ring-1 ring-inset ${BADGE_TONES[tone]}`}
    >
      {children}
    </span>
  );
}

/** An error surface. Always `role="alert"` — every error branch in the app
 * goes through here, so none of them can quietly not announce itself. */
export function Alert({ children }: { children: ReactNode }) {
  return (
    <p
      role="alert"
      className="rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-800"
    >
      {children}
    </p>
  );
}

/** The "waiting on a webhook" surface (D5). Amber, not red: nothing has
 * failed yet, and the capped message tells you what to check when it has. */
export function Notice({ children, busy = false }: { children: ReactNode; busy?: boolean }) {
  return (
    <p
      {...(busy ? { "aria-busy": "true" } : { role: "alert" })}
      className="flex items-center gap-2.5 rounded-lg border border-amber-200 bg-amber-50 px-4 py-3 text-sm text-amber-900"
    >
      {busy && <Spinner />}
      {children}
    </p>
  );
}

export function Spinner({ className = "h-4 w-4" }: { className?: string }) {
  return (
    <svg
      className={`${className} shrink-0 animate-spin text-current opacity-70`}
      viewBox="0 0 24 24"
      fill="none"
      aria-hidden="true"
    >
      <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="3" />
      <path
        className="opacity-90"
        fill="currentColor"
        d="M4 12a8 8 0 0 1 8-8v3a5 5 0 0 0-5 5H4z"
      />
    </svg>
  );
}

/** The loading branch of a query. Keeps `aria-busy` on the element that
 * actually carries the message. */
export function Loading({ children }: { children: ReactNode }) {
  return (
    <p aria-busy="true" className="flex items-center gap-2.5 text-sm text-slate-500">
      <Spinner />
      {children}
    </p>
  );
}

export function EmptyState({ children }: { children: ReactNode }) {
  return (
    <p className="rounded-xl border border-dashed border-slate-300 bg-slate-50/60 px-4 py-8 text-center text-sm text-slate-500">
      {children}
    </p>
  );
}

/** A row in one of the two lists (cards, invoices). Not a `<li>` itself so
 * the lists keep control of their own list semantics. */
export function Row({ children }: { children: ReactNode }) {
  return (
    <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2 rounded-xl border border-slate-200 bg-white px-4 py-3">
      {children}
    </div>
  );
}
