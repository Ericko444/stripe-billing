import { ApiProblem } from "../../host/api/apiClient";
import { usePlans, useChangePlan } from "../hooks/usePlans";
import { formatMoney } from "../../money";
import { Alert, Badge, Button, Loading } from "../../ui/primitives";

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
    return <Loading>Loading plans…</Loading>;
  }

  if (plansQuery.isError) {
    const detail =
      plansQuery.error instanceof ApiProblem ? plansQuery.error.detail : "Something went wrong.";
    return <Alert>Could not load plans: {detail}</Alert>;
  }

  return (
    <div className="stack">
      <span className="panel-label">Plan</span>
      <div className="plan-grid">
        {plansQuery.data.map((plan) => {
          const isCurrent = plan.id === currentPlanId;
          return (
            <div key={plan.id} className={`plan-card${isCurrent ? " is-current" : ""}`}>
              <div className="plan-card-head">
                <span className="plan-name">{plan.name}</span>
                {isCurrent && <Badge tone="accent">current plan</Badge>}
              </div>
              <div className="plan-price">{formatMoney(plan.amount)}</div>
              {!isCurrent && (
                <Button
                  variant="secondary"
                  block
                  disabled={changePlan.isPending}
                  onClick={() => changePlan.mutate(plan.id)}
                >
                  {changePlan.isPending ? "Switching…" : `Switch to ${plan.name}`}
                </Button>
              )}
            </div>
          );
        })}
      </div>
      {changePlan.isError && (
        <Alert>
          Could not change plan:{" "}
          {changePlan.error instanceof ApiProblem
            ? changePlan.error.detail
            : "Something went wrong."}
        </Alert>
      )}
      {changePlan.isSuccess && (
        <span style={{ fontSize: "13px", color: "var(--color-positive)" }}>Plan changed.</span>
      )}
    </div>
  );
}
