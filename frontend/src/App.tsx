import { TenantSwitcher } from "./host/auth/TenantSwitcher";
import { SubscriptionPanel } from "./billing/components/SubscriptionPanel";
import { InvoiceList } from "./billing/components/InvoiceList";
import { CheckoutReturn } from "./billing/components/CheckoutReturn";

/** No router library (Tech Stack decision): one static return path from
 * Stripe Checkout, checked once against the real navigation, not client
 * routing. */
export default function App() {
  if (window.location.pathname === "/checkout/return") {
    return (
      <main>
        <h1>Billing demo</h1>
        <CheckoutReturn />
      </main>
    );
  }

  return (
    <main>
      <h1>Billing demo</h1>
      <TenantSwitcher />
      <SubscriptionPanel />
      <InvoiceList />
    </main>
  );
}
