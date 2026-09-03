import { describe, expect, test } from "vitest";
import { formatMoney } from "./money";

describe("formatMoney", () => {
  test("formats a typical amount", () => {
    expect(formatMoney({ amount_minor: 1999, currency: "EUR" })).toBe("19.99 EUR");
  });

  test("formats a negative amount", () => {
    expect(formatMoney({ amount_minor: -500, currency: "USD" })).toBe("-5.00 USD");
  });

  test("formats an amount under one major unit", () => {
    expect(formatMoney({ amount_minor: 5, currency: "EUR" })).toBe("0.05 EUR");
  });

  test("groups large amounts", () => {
    expect(formatMoney({ amount_minor: 123456, currency: "USD" })).toBe("1,234.56 USD");
  });
});
