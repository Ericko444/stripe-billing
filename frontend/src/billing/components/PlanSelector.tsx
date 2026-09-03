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
    <div className="space-y-3">
      <h3 className="text-xs font-semibold uppercase tracking-[0.08em] text-slate-400">Plan</h3>
      <ul className="grid gap-3 sm:grid-cols-2">
        {plansQuery.data.map((plan) => {
          const isCurrent = plan.id === currentPlanId;
          return (
            <li key={plan.id}>
              <div
                className={`flex h-full flex-col justify-between gap-4 rounded-xl border p-4 transition-colors ${
                  isCurrent
                    ? "border-indigo-300 bg-indigo-50/60 ring-1 ring-indigo-200"
                    : "border-slate-200 hover:border-indigo-300"
                }`}
              >
                <div>
                  <div className="flex items-center justify-between gap-2">
                    <p className="font-medium text-slate-900">{plan.name}</p>
                    {isCurrent && <Badge tone="green">current plan</Badge>}
                  </div>
                  <p className="mt-0.5 text-2xl font-semibold tracking-tight text-slate-900">
                    {formatMoney(plan.amount)}
                  </p>
                </div>
                {!isCurrent && (
                  <Button
                    variant="secondary"
                    disabled={changePlan.isPending}
                    onClick={() => changePlan.mutate(plan.id)}
                  >
                    {changePlan.isPending ? "Switching…" : `Switch to ${plan.name}`}
                  </Button>
                )}
              </div>
            </li>
          );
        })}
      </ul>
      {changePlan.isError && (
        <Alert>
          Could not change plan:{" "}
          {changePlan.error instanceof ApiProblem
            ? changePlan.error.detail
            : "Something went wrong."}
        </Alert>
      )}
      {changePlan.isSuccess && (
        <p className="text-sm font-medium text-emerald-700">Plan changed.</p>
      )}
    </div>
  );
}
