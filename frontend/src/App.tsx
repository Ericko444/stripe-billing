import { TenantSwitcher } from "./host/auth/TenantSwitcher";
import { SubscriptionPanel } from "./billing/components/SubscriptionPanel";
import { InvoiceList } from "./billing/components/InvoiceList";

export default function App() {
  return (
    <main>
      <h1>Billing demo</h1>
      <TenantSwitcher />
      <SubscriptionPanel />
      <InvoiceList />
    </main>
  );
}
