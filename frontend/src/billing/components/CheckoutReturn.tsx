import { useEffect, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useAuth } from "../../host/auth/AuthContext";
import { billingKeys } from "../api/keys";
import { useSubscription } from "../hooks/useSubscription";
import { CheckCircle, Spinner } from "../../ui/primitives";

const POLL_INTERVAL_MS = 2000;
const MAX_ATTEMPTS = 10;

/**
 * D5 / §11.5: the redirect is a UX signal, the webhook is the truth. This
 * page never reads Stripe's own query params as proof of anything -- it
 * polls `GET /subscription` until the mirror itself says `active`, capped
 * so a stalled `stripe listen` fails loud instead of spinning forever.
 */
export function CheckoutReturn() {
  const { tenantId } = useAuth();
  const queryClient = useQueryClient();
  const query = useSubscription();
  const [attempts, setAttempts] = useState(0);

  // Once, on arrival: the redirect itself is a strong signal something
  // changed, so don't wait for the first poll tick to find out.
  useEffect(() => {
    queryClient.invalidateQueries({ queryKey: billingKeys.subscription(tenantId) });
  }, []);

  const isActive = query.data?.status === "active";

  useEffect(() => {
    if (isActive || attempts >= MAX_ATTEMPTS) {
      return;
    }
    const timer = setTimeout(() => {
      setAttempts((n) => n + 1);
      void query.refetch();
    }, POLL_INTERVAL_MS);
    return () => clearTimeout(timer);
  }, [attempts, isActive]);

  if (isActive) {
    return (
      <div className="card elev-md checkout-card">
        <CheckCircle size={26} />
        <div className="lead">Subscription active.</div>
        <div className="panel-sub">Confirmed by the webhook, not by the redirect.</div>
        <a href="/" className="btn btn-primary" style={{ marginTop: "4px" }}>
          Continue
        </a>
      </div>
    );
  }

  if (attempts >= MAX_ATTEMPTS) {
    return (
      <div className="card elev-md checkout-card" role="alert">
        <svg width="24" height="24" viewBox="0 0 24 24" fill="none" aria-hidden="true">
          <path
            d="M12 3 2 20h20L12 3Z"
            stroke="var(--color-caution)"
            strokeWidth="2"
            strokeLinejoin="round"
          />
          <path
            d="M12 10v4M12 17h.01"
            stroke="var(--color-caution)"
            strokeWidth="2"
            strokeLinecap="round"
          />
        </svg>
        <div style={{ fontSize: "14px", color: "var(--color-caution)" }}>
          Still waiting for Stripe to confirm. Is <code>stripe listen</code> running?
        </div>
      </div>
    );
  }

  return (
    <div className="card elev-md checkout-card" aria-busy="true">
      <Spinner size={22} />
      <div className="lead">Confirming your subscription…</div>
      <div className="panel-sub">Waiting for Stripe's webhook to reach the mirror.</div>
    </div>
  );
}
