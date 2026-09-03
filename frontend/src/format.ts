/** Timestamps cross the wire as RFC 3339 strings. Rendering them raw is
 * accurate and unreadable; this makes them legible on a screen share.
 *
 * The locale is pinned to `en-US` for the same reason D10 pins
 * `formatMoney`'s: the machine default is not the same everywhere, and a
 * demo's dates should not change shape depending on who runs it. */
const DATE_FORMAT = new Intl.DateTimeFormat("en-US", {
  year: "numeric",
  month: "short",
  day: "numeric",
});

export function formatDate(iso: string): string {
  const parsed = new Date(iso);
  // An unparseable timestamp is a wire bug, not something to render as
  // "Invalid Date" — show what the server actually sent instead.
  return Number.isNaN(parsed.getTime()) ? iso : DATE_FORMAT.format(parsed);
}
