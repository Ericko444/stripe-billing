import { TenantSwitcher } from "./host/auth/TenantSwitcher";
import { SubscriptionPanel } from "./billing/components/SubscriptionPanel";

export default function App() {
  return (
    <main>
      <h1>Billing demo</h1>
      <TenantSwitcher />
      <SubscriptionPanel />
    </main>
  );
}
