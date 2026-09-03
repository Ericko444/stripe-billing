import type { ReactNode } from "react";
import { TenantSwitcher } from "./host/auth/TenantSwitcher";
import { SubscriptionPanel } from "./billing/components/SubscriptionPanel";
import { InvoiceList } from "./billing/components/InvoiceList";
import { CheckoutReturn } from "./billing/components/CheckoutReturn";
import { PaymentMethodPanel } from "./billing/components/PaymentMethodPanel";

function Header({ children }: { children?: ReactNode }) {
  return (
    <header className="sticky top-0 z-10 border-b border-slate-200 bg-white/85 backdrop-blur">
      <div className="mx-auto flex max-w-5xl flex-wrap items-center justify-between gap-4 px-6 py-4">
        <div className="flex items-center gap-3">
          <span className="flex h-9 w-9 items-center justify-center rounded-xl bg-indigo-600 text-sm font-bold text-white shadow-sm">
            B
          </span>
          <div>
            <h1 className="text-base font-semibold leading-tight text-slate-900">Billing demo</h1>
            <p className="text-xs leading-tight text-slate-500">
              Subscriptions, invoices and cards, mirrored from Stripe
            </p>
          </div>
        </div>
        {children}
      </div>
    </header>
  );
}

/** No router library (Tech Stack decision): one static return path from
 * Stripe Checkout, checked once against the real navigation, not client
 * routing. */
export default function App() {
  if (window.location.pathname === "/checkout/return") {
    return (
      <div className="min-h-screen">
        <Header />
        <main className="mx-auto max-w-xl px-6 py-16">
          <CheckoutReturn />
        </main>
      </div>
    );
  }

  return (
    <div className="min-h-screen">
      <Header>
        <TenantSwitcher />
      </Header>
      <main className="mx-auto max-w-5xl space-y-6 px-6 py-10">
        <SubscriptionPanel />
        <div className="grid gap-6 lg:grid-cols-2">
          <PaymentMethodPanel />
          <InvoiceList />
        </div>
      </main>
    </div>
  );
}
