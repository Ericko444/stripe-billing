/**
 * S5: money crosses the wire and the UI as `{ amount_minor, currency }`,
 * never a float. `formatMoney` divides by integer truncation and modulo --
 * no value with a fractional part is ever constructed.
 *
 * Locale is pinned to `en-US` rather than the browser default: an
 * environment whose default locale groups with a narrow no-break space
 * (e.g. `fr-FR`) is correct behavior for `Intl.NumberFormat`, but
 * unpredictable to demo from and a false diff between two runs on
 * different machines.
 */
export interface Money {
  amount_minor: number;
  currency: string;
}

export function formatMoney({ amount_minor, currency }: Money): string {
  const negative = amount_minor < 0;
  const abs = Math.abs(amount_minor);
  const major = Math.trunc(abs / 100); // integer, never a fractional value
  const minor = abs % 100;
  const digits = new Intl.NumberFormat("en-US", { useGrouping: true }).format(major);
  const symbol = currency.toUpperCase();
  return `${negative ? "-" : ""}${digits}.${String(minor).padStart(2, "0")} ${symbol}`;
}
