import type { ReactNode } from "react";
import { TenantSwitcher } from "./host/auth/TenantSwitcher";
import { SubscriptionPanel } from "./billing/components/SubscriptionPanel";
import { InvoiceList } from "./billing/components/InvoiceList";
import { CheckoutReturn } from "./billing/components/CheckoutReturn";
import { PaymentMethodPanel } from "./billing/components/PaymentMethodPanel";
import { BrandMark } from "./ui/primitives";

function Nav({ children }: { children?: ReactNode }) {
  return (
    <header className="nav">
      <BrandMark size={20} />
      <div style={{ display: "flex", flexDirection: "column", marginRight: "auto" }}>
        <span className="nav-brand">Billing demo</span>
        <span className="nav-sub">A standalone mirror of one tenant's Stripe billing state</span>
      </div>
      {children}
    </header>
  );
}

/** No router library (Tech Stack decision): one static return path from
 * Stripe Checkout, checked once against the real navigation, not client
 * routing. */
export default function App() {
  if (window.location.pathname === "/checkout/return") {
    return (
      <div className="app">
        <Nav />
        <main className="checkout-main">
          <CheckoutReturn />
        </main>
      </div>
    );
  }

  return (
    <div className="app">
      <Nav>
        <TenantSwitcher />
      </Nav>
      <main className="app-main">
        <SubscriptionPanel />
        <div className="grid-2">
          <PaymentMethodPanel />
          <InvoiceList />
        </div>
      </main>
    </div>
  );
}
