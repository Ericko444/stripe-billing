import { ApiProblem } from "../../host/api/apiClient";
import { useCancelSubscription, useSubscription } from "../hooks/useSubscription";
import { PlanSelector } from "./PlanSelector";

/** U1, D3's five-way state table. `incomplete` offers cancel directly here
 * (D2's demo path depends on it); `active` renders `PlanSelector` (U2).
 * Starting Checkout for the no-subscription state is wired in Task 9. */
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
