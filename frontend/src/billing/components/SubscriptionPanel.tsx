import { ApiProblem } from "../../host/api/apiClient";
import {
  useCancelSubscription,
  useStartCheckoutSession,
  useSubscription,
} from "../hooks/useSubscription";
import { usePlans } from "../hooks/usePlans";
import { PlanSelector } from "./PlanSelector";
import { formatMoney } from "../../money";
import { rememberTenantForReturn, useAuth } from "../../host/auth/AuthContext";

/** The no-subscription state's plan choice, starting a Checkout Session.
 * Redirects to Stripe's hosted page on success -- there is nothing to show
 * locally afterward until `CheckoutReturn`'s poll confirms the webhook. */
function StartSubscription() {
  const { tenantId } = useAuth();
  const plansQuery = usePlans();
  const startCheckout = useStartCheckoutSession();

  if (plansQuery.isPending) {
    return <p>Loading plans…</p>;
  }
  if (plansQuery.isError) {
    const detail =
      plansQuery.error instanceof ApiProblem ? plansQuery.error.detail : "Something went wrong.";
    return <p role="alert">Could not load plans: {detail}</p>;
  }

  return (
    <ul>
      {plansQuery.data.map((plan) => (
        <li key={plan.id}>
          {plan.name} — {formatMoney(plan.amount)}
          <button
            type="button"
            disabled={startCheckout.isPending}
            onClick={() =>
              startCheckout.mutate(plan.id, {
                onSuccess: (data) => {
                  rememberTenantForReturn(tenantId);
                  window.location.href = data.url;
                },
              })
            }
          >
            Subscribe
          </button>
        </li>
      ))}
      {startCheckout.isError && (
        <p role="alert">
          Could not start checkout:{" "}
          {startCheckout.error instanceof ApiProblem
            ? startCheckout.error.detail
            : "Something went wrong."}
        </p>
      )}
    </ul>
  );
}

/** U1, D3's five-way state table. `incomplete` offers cancel directly here
 * (D2's demo path depends on it); `active` renders `PlanSelector` (U2);
 * no-subscription offers a plan choice that starts Checkout. */
export function SubscriptionPanel() {
  const query = useSubscription();
  const cancelSubscription = useCancelSubscription();

  if (query.isPending) {
    return <section aria-busy="true">Loading your subscription…</section>;
  }

  if (query.isError) {
    const detail =
      query.error instanceof ApiProblem ? query.error.detail : "Something went wrong.";
    return <section role="alert">Could not load your subscription: {detail}</section>;
  }

  const subscription = query.data;

  if (subscription === null) {
    return (
      <section>
        <p>You don't have a subscription yet.</p>
        <StartSubscription />
      </section>
    );
  }

  if (subscription.status === "incomplete") {
    return (
      <section>
        <p>Your subscription was created but never paid for.</p>
        <button
          type="button"
          disabled={cancelSubscription.isPending}
          onClick={() => cancelSubscription.mutate(subscription.id)}
        >
          Cancel
        </button>
        {cancelSubscription.isError && (
          <p role="alert">
            Could not cancel:{" "}
            {cancelSubscription.error instanceof ApiProblem
              ? cancelSubscription.error.detail
              : "Something went wrong."}
          </p>
        )}
      </section>
    );
  }

  return (
    <section>
      <p>
        Subscription status: <strong>{subscription.status}</strong>
      </p>
      {subscription.status === "active" && (
        <PlanSelector subscriptionId={subscription.id} currentPlanId={subscription.plan_id} />
      )}
    </section>
  );
}
