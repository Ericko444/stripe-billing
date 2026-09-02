import { ApiProblem } from "../../host/api/apiClient";
import { useSubscription } from "../hooks/useSubscription";

/**
 * U1, D3's five-way state table. The write actions each non-`active`,
 * non-error state implies -- start Checkout, cancel -- are wired in later
 * (Tasks 8/9); this component only reads and displays the wire state.
 */
export function SubscriptionPanel() {
  const query = useSubscription();

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
      </section>
    );
  }

  return (
    <section>
      <p>
        Subscription status: <strong>{subscription.status}</strong>
      </p>
    </section>
  );
}
