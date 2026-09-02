import { useEffect, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useAuth } from "../../host/auth/AuthContext";
import { billingKeys } from "../api/keys";
import { useSubscription } from "../hooks/useSubscription";

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
      <section>
        <p>Subscription active.</p>
        <a href="/">Continue</a>
      </section>
    );
  }

  if (attempts >= MAX_ATTEMPTS) {
    return (
      <section role="alert">
        <p>Still waiting for Stripe to confirm. Is `stripe listen` running?</p>
      </section>
    );
  }

  return <section aria-busy="true">Confirming your subscription…</section>;
}
