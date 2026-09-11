import type { ButtonHTMLAttributes, ReactNode } from "react";

/**
 * Presentational primitives for the four panels, built on the Nocturne
 * classes in `index.css` (the "Billing Demo UI Spec" design system). Each
 * one exists because three or more call sites want it; none knows anything
 * about billing.
 *
 * The a11y attributes the tests and screen readers rely on (`role="alert"`,
 * `aria-busy`) are carried here rather than re-typed at every call site.
 */

/* ── Icons — the spec's stroke set, on a 24px grid ────────────────────── */

export function BrandMark({ size = 20 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 256 256" fill="var(--color-accent)" aria-hidden="true">
      <rect x="32" y="32" width="192" height="192" rx="40" />
    </svg>
  );
}

export function Spinner({ size = 16, tone = "accent" }: { size?: number; tone?: "accent" | "current" }) {
  const track = tone === "current" ? "currentColor" : "var(--color-neutral-700)";
  const arc = tone === "current" ? "currentColor" : "var(--color-accent)";
  return (
    <svg
      className="spin"
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      aria-hidden="true"
    >
      <circle cx="12" cy="12" r="9" stroke={track} strokeWidth="2.5" opacity={tone === "current" ? 0.3 : 1} />
      <path d="M21 12a9 9 0 0 0-9-9" stroke={arc} strokeWidth="2.5" strokeLinecap="round" />
    </svg>
  );
}

function AlertIcon({ size = 16 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <circle cx="12" cy="12" r="9" stroke="currentColor" strokeWidth="2" />
      <path d="M12 8v5M12 16h.01" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
    </svg>
  );
}

function WarningIcon({ size = 16 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <path d="M12 3 2 20h20L12 3Z" stroke="currentColor" strokeWidth="2" strokeLinejoin="round" />
      <path d="M12 10v4M12 17h.01" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
    </svg>
  );
}

export function CheckCircle({ size = 26 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <circle cx="12" cy="12" r="10" stroke="var(--color-positive)" strokeWidth="2" />
      <path
        d="M8 12.5 11 15.5 16 9"
        stroke="var(--color-positive)"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

/* ── Card ────────────────────────────────────────────────────────────── */

/** A titled panel. `prominent` is the Subscription panel's larger title +
 * elevation; the other panels use the default. `action` sits opposite the
 * title. */
export function Card({
  title,
  subtitle,
  action,
  prominent = false,
  children,
}: {
  title: string;
  subtitle?: string;
  action?: ReactNode;
  prominent?: boolean;
  children: ReactNode;
}) {
  return (
    <section
      className={`card ${prominent ? "elev-md" : "elev-sm"}`}
      style={{ padding: prominent ? "20px" : "18px", gap: "14px" }}
    >
      <div className="panel-head">
        <div>
          <h2 className={`panel-title${prominent ? " is-prominent" : ""}`}>{title}</h2>
          {subtitle && <p className="panel-sub">{subtitle}</p>}
        </div>
        {action}
      </div>
      {children}
    </section>
  );
}

/* ── Button ──────────────────────────────────────────────────────────── */

const VARIANTS = {
  primary: "btn-primary",
  secondary: "btn-secondary",
  destructive: "btn-destructive",
  ghost: "btn-ghost",
} as const;

export function Button({
  variant = "secondary",
  size,
  block = false,
  className = "",
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: keyof typeof VARIANTS;
  size?: "sm";
  block?: boolean;
}) {
  const cls = ["btn", VARIANTS[variant], size === "sm" && "btn-sm", block && "btn-block", className]
    .filter(Boolean)
    .join(" ");
  return <button className={cls} {...props} />;
}

/* ── Badge ───────────────────────────────────────────────────────────── */

const BADGE_TONES = {
  positive: "tag-positive",
  caution: "tag-caution",
  negative: "tag-negative",
  neutral: "tag-neutral",
  accent: "tag-accent",
} as const;

export function Badge({
  tone = "neutral",
  children,
}: {
  tone?: keyof typeof BADGE_TONES;
  children: ReactNode;
}) {
  return <span className={`tag ${BADGE_TONES[tone]}`}>{children}</span>;
}

/* ── Feedback surfaces ───────────────────────────────────────────────── */

/** An error surface — always `role="alert"`, so no error branch can quietly
 * fail to announce itself. */
export function Alert({ children }: { children: ReactNode }) {
  return (
    <p role="alert" className="alert">
      <AlertIcon />
      <span>{children}</span>
    </p>
  );
}

/** The "waiting on a webhook" surface. `busy` while polling (spinner),
 * plain caution once the poll has capped (warning triangle). */
export function Notice({ children, busy = false }: { children: ReactNode; busy?: boolean }) {
  return (
    <p {...(busy ? { "aria-busy": "true" } : { role: "alert" })} className="notice">
      {busy ? <Spinner size={15} tone="current" /> : <WarningIcon size={15} />}
      <span>{children}</span>
    </p>
  );
}

/** The loading branch of a query — keeps `aria-busy` on the element that
 * carries the message. */
export function Loading({ children }: { children: ReactNode }) {
  return (
    <p aria-busy="true" className="loading-line">
      <Spinner />
      <span>{children}</span>
    </p>
  );
}

export function EmptyState({ children }: { children: ReactNode }) {
  return <p className="empty-line">{children}</p>;
}

/** A row in one of the two lists (cards, invoices). Not a `<li>` itself so
 * the lists keep control of their own list semantics. */
export function Row({ children }: { children: ReactNode }) {
  return <div className="row">{children}</div>;
}
