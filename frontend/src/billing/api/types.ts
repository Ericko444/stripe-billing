/**
 * Wire types, hand-mirrored from `crates/api/src/dto.rs`. That file is the
 * source of truth (P2/D8) -- every interface here corresponds to one `*Dto`
 * struct or request body there, kept in this one file so a diff against it
 * is the whole surface.
 */

export interface MoneyDto {
  amount_minor: number;
  currency: string;
}

export interface PlanDto {
  id: string;
  name: string;
  amount: MoneyDto;
  created_at: string;
}

/** `dto.rs`'s `SubscriptionStatus::as_str` -- five variants, including the
 * terminal `incomplete_expired` (`crates/domain/src/subscription.rs`). */
export type SubscriptionStatus =
  | "active"
  | "past_due"
  | "canceled"
  | "incomplete"
  | "incomplete_expired";

export interface SubscriptionDto {
  id: string;
  plan_id: string;
  status: SubscriptionStatus;
  current_period_start: string;
  current_period_end: string;
  cancel_at_period_end: boolean;
  created_at: string;
}

export interface ChangePlanRequest {
  plan_id: string;
}

export interface CancelRequest {
  at_period_end: boolean;
}

export interface CheckoutSessionRequest {
  plan_id: string;
}

export interface CheckoutSessionDto {
  url: string;
}

export interface PaymentMethodDto {
  id: string;
  brand: string;
  last4: string;
  is_default: boolean;
  created_at: string;
}

export interface SetupIntentDto {
  client_secret: string;
}

export type InvoiceStatus = "open" | "paid" | "failed";

export interface InvoiceDto {
  id: string;
  subscription_id: string | null;
  amount: MoneyDto;
  status: InvoiceStatus;
  created_at: string;
}

/** `next` is **absent** from the JSON on the last page, not `null` --
 * `#[serde(skip_serializing_if = "Option::is_none")]` in `dto.rs`. Typed as
 * optional (`next?`), not nullable, to match. */
export interface InvoicePageDto {
  items: InvoiceDto[];
  next?: string;
}
