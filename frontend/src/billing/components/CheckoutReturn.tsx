import { useEffect, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useAuth } from "../../host/auth/AuthContext";
import { billingKeys } from "../api/keys";
import { useSubscription } from "../hooks/useSubscription";
import { Card, Notice, Spinner } from "../../ui/primitives";

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
      <Card title="Checkout">
        <div className="space-y-5 text-center">
          <span className="mx-auto flex h-12 w-12 items-center justify-center rounded-full bg-emerald-50 text-emerald-600 ring-1 ring-emerald-600/20">
            <svg
              className="h-6 w-6"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="2.5"
              strokeLinecap="round"
              strokeLinejoin="round"
              aria-hidden="true"
            >
              <path d="M20 6 9 17l-5-5" />
            </svg>
          </span>
          <div>
            <p className="text-lg font-semibold text-slate-900">Subscription active.</p>
            <p className="mt-1 text-sm text-slate-500">
              Confirmed by the webhook, not by the redirect.
            </p>
          </div>
          <a
            href="/"
            className="inline-flex items-center justify-center rounded-lg bg-indigo-600 px-4 py-2 text-sm font-medium text-white shadow-sm transition-colors hover:bg-indigo-500 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-indigo-600"
          >
            Continue
          </a>
        </div>
      </Card>
    );
  }

  if (attempts >= MAX_ATTEMPTS) {
    return (
      <Card title="Checkout">
        <Notice>
          Still waiting for Stripe to confirm. Is <code>stripe listen</code> running?
        </Notice>
      </Card>
    );
  }

  return (
    <Card title="Checkout">
      <div
        aria-busy="true"
        className="flex flex-col items-center gap-4 py-6 text-center text-slate-600"
      >
        <Spinner className="h-8 w-8 text-indigo-600" />
        <div>
          <p className="font-medium text-slate-900">Confirming your subscription…</p>
          <p className="mt-1 text-sm text-slate-500">
            Waiting for Stripe's webhook to reach the mirror.
          </p>
        </div>
      </div>
    </Card>
  );
}
