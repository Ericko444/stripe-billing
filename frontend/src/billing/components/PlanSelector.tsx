import { ApiProblem } from "../../host/api/apiClient";
import { usePlans, useChangePlan } from "../hooks/usePlans";
import { formatMoney } from "../../money";

/** U2. Rendered only for an `active` subscription (D3) -- changing plan on
 * one that was never paid for is not a meaningful action, and cancelling an
 * `incomplete` one is `SubscriptionPanel`'s job instead. */
export function PlanSelector({
  subscriptionId,
  currentPlanId,
}: {
  subscriptionId: string;
  currentPlanId: string;
}) {
  const plansQuery = usePlans();
  const changePlan = useChangePlan(subscriptionId);

  if (plansQuery.isPending) {
    return <section aria-busy="true">Loading plans…</section>;
  }

  if (plansQuery.isError) {
    const detail =
      plansQuery.error instanceof ApiProblem ? plansQuery.error.detail : "Something went wrong.";
    return <section role="alert">Could not load plans: {detail}</section>;
  }

  return (
    <section>
      <ul>
        {plansQuery.data.map((plan) => {
          const isCurrent = plan.id === currentPlanId;
          return (
            <li key={plan.id}>
              {plan.name} — {formatMoney(plan.amount)}
              {isCurrent ? (
                " (current plan)"
              ) : (
                <button
                  type="button"
                  disabled={changePlan.isPending}
                  onClick={() => changePlan.mutate(plan.id)}
                >
                  Switch to {plan.name}
                </button>
              )}
            </li>
          );
        })}
      </ul>
      {changePlan.isError && (
        <p role="alert">
          Could not change plan:{" "}
          {changePlan.error instanceof ApiProblem
            ? changePlan.error.detail
            : "Something went wrong."}
        </p>
      )}
      {changePlan.isSuccess && <p>Plan changed.</p>}
    </section>
  );
}
