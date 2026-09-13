import type { ReactNode } from "react";
import { useSessionState } from "./host/auth/AuthContext";
import { TenantSwitcher } from "./host/auth/TenantSwitcher";
import { LoginForm } from "./identity/components/LoginForm";
import { TenantPicker } from "./identity/components/TenantPicker";
import { SubscriptionPanel } from "./billing/components/SubscriptionPanel";
import { InvoiceList } from "./billing/components/InvoiceList";
import { CheckoutReturn } from "./billing/components/CheckoutReturn";
import { PaymentMethodPanel } from "./billing/components/PaymentMethodPanel";
import { Alert, BrandMark, Button, Loading, Notice } from "./ui/primitives";

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

/** A centred single-card page: every step before billing is one of these. */
function Centred({ children }: { children: ReactNode }) {
  return (
    <div className="app">
      <Nav />
      <main className="checkout-main">{children}</main>
    </div>
  );
}

/** The signed-in navigation: who, which tenant, and the way out. */
function SignedInNav() {
  const { me, logout, isLoggingOut, switchProblem } = useSessionState();
  return (
    <Nav>
      {switchProblem && <Alert>Could not switch tenant. Try again.</Alert>}
      <TenantSwitcher />
      <span className="muted" style={{ fontSize: "13px" }}>
        {me?.email}
      </span>
      <Button size="sm" variant="ghost" onClick={logout} disabled={isLoggingOut}>
        {isLoggingOut ? "Logging out…" : "Log out"}
      </Button>
    </Nav>
  );
}

/**
 * The shell: which screen follows from the session's state, and nothing
 * else. Billing is rendered only in the "ready" branch -- the one place a
 * tenant is guaranteed -- so no billing component ever has to ask.
 *
 * No router library (Phase 5's decision holds): the one static return path
 * from Stripe Checkout is still read off the real navigation, and it too
 * waits for a tenant-scoped session. The cookie survives the round trip, so
 * the user comes back to the same tenant they left from.
 */
export default function App() {
  const session = useSessionState();

  switch (session.status) {
    case "loading":
      return (
        <Centred>
          <Loading>Checking your session…</Loading>
        </Centred>
      );

    case "unavailable":
      return (
        <Centred>
          <div className="card elev-md stack" style={{ padding: "24px", maxWidth: "380px" }}>
            <Alert>The server could not be reached.</Alert>
            <Button variant="primary" onClick={session.retry}>
              Try again
            </Button>
          </div>
        </Centred>
      );

    case "anonymous":
      return (
        <Centred>
          <LoginForm
            notice={
              session.sessionEnded ? (
                <Notice>Your session ended. Log in again to continue.</Notice>
              ) : undefined
            }
          />
        </Centred>
      );

    case "choosingTenant":
      return (
        <Centred>
          <TenantPicker
            memberships={session.me?.memberships ?? []}
            onPick={session.switchTenant}
            pending={session.isSwitching}
            problem={session.switchProblem}
            onLogout={session.logout}
          />
        </Centred>
      );

    case "ready":
      if (window.location.pathname === "/checkout/return") {
        return (
          <div className="app">
            <SignedInNav />
            <main className="checkout-main">
              <CheckoutReturn />
            </main>
          </div>
        );
      }
      return (
        <div className="app">
          <SignedInNav />
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
}
