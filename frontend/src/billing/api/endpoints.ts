import { apiRequest } from "../../host/api/apiClient";
import type {
  CancelRequest,
  ChangePlanRequest,
  CheckoutSessionDto,
  CheckoutSessionRequest,
  InvoiceDto,
  InvoicePageDto,
  PaymentMethodDto,
  PlanDto,
  SetupIntentDto,
  SubscriptionDto,
} from "./types";

/**
 * One function per billing route -- eleven of the API's thirteen.
 * `POST /webhooks/stripe` has no frontend caller by design; `POST
 * /demo/token` is a host, not a billing, concern and stays in
 * `host/auth/AuthContext.tsx`, which already calls it directly for the
 * reason given there.
 */

export function getPlans(): Promise<PlanDto[]> {
  return apiRequest("/plans");
}

/** `GET /subscription` returns `200 null` for "never subscribed" --
 * a normal state, not a 404 (`crates/api/src/routes/subscription.rs`). */
export function getSubscription(): Promise<SubscriptionDto | null> {
  return apiRequest("/subscription");
}

export function startCheckoutSession(body: CheckoutSessionRequest): Promise<CheckoutSessionDto> {
  return apiRequest("/subscriptions/checkout-session", {
    method: "POST",
    body: JSON.stringify(body),
  });
}

export function changePlan(
  subscriptionId: string,
  body: ChangePlanRequest,
): Promise<SubscriptionDto> {
  return apiRequest(`/subscriptions/${subscriptionId}/change-plan`, {
    method: "POST",
    body: JSON.stringify(body),
  });
}

export function cancelSubscription(
  subscriptionId: string,
  body: CancelRequest,
): Promise<SubscriptionDto> {
  return apiRequest(`/subscriptions/${subscriptionId}/cancel`, {
    method: "POST",
    body: JSON.stringify(body),
  });
}

export function getInvoices(after?: string): Promise<InvoicePageDto> {
  const query = after ? `?after=${encodeURIComponent(after)}` : "";
  return apiRequest(`/invoices${query}`);
}

export function getInvoice(invoiceId: string): Promise<InvoiceDto> {
  return apiRequest(`/invoices/${invoiceId}`);
}

export function getPaymentMethods(): Promise<PaymentMethodDto[]> {
  return apiRequest("/payment-methods");
}

export function createSetupIntent(): Promise<SetupIntentDto> {
  return apiRequest("/payment-methods/setup-intent", { method: "POST" });
}

export function setDefaultPaymentMethod(paymentMethodId: string): Promise<PaymentMethodDto> {
  return apiRequest(`/payment-methods/${paymentMethodId}/default`, { method: "POST" });
}

export function removePaymentMethod(paymentMethodId: string): Promise<void> {
  return apiRequest(`/payment-methods/${paymentMethodId}`, { method: "DELETE" });
}
