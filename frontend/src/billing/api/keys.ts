/**
 * Every key carries `tenantId` first, so switching tenants can
 * never serve a stale value from the previous one even before
 * `queryClient.clear()` runs. No hook is allowed to build a key by hand.
 */
export const billingKeys = {
  subscription: (tenantId: string) => ["subscription", tenantId] as const,
  plans: (tenantId: string) => ["plans", tenantId] as const,
  invoices: (tenantId: string, after?: string) =>
    ["invoices", tenantId, after ?? null] as const,
  paymentMethods: (tenantId: string) => ["paymentMethods", tenantId] as const,
};
