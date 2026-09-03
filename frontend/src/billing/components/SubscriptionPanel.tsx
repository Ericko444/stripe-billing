import { ApiProblem } from "../../host/api/apiClient";
import {
  useCancelSubscription,
  useStartCheckoutSession,
  useSubscription,
} from "../hooks/useSubscription";
import { usePlans } from "../hooks/usePlans";
import { PlanSelector } from "./PlanSelector";
import { formatMoney } from "../../money";
import { formatDate } from "../../format";
import { rememberTenantForReturn, useAuth } from "../../host/auth/AuthContext";
import { Alert, Badge, Button, Card, EmptyState, Loading, Row } from "../../ui/primitives";
import type { SubscriptionStatus } from "../api/types";

const STATUS_TONE: Record<SubscriptionStatus, "green" | "amber" | "slate" | "red"> = {
  active: "green",
  past_due: "red",
  incomplete: "amber",
  incomplete_expired: "slate",
  canceled: "slate",
};

/** The no-subscription state's plan choice, starting a Checkout Session.
 * Redirects to Stripe's hosted page on success -- there is nothing to show
 * locally afterward until `CheckoutReturn`'s poll confirms the webhook. */
function StartSubscription() {
  const { tenantId } = useAuth();
  const plansQuery = usePlans();
  const startCheckout = useStartCheckoutSession();

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
      <ul className="grid gap-3 sm:grid-cols-2">
        {plansQuery.data.map((plan) => (
          <li key={plan.id}>
            <div className="flex h-full flex-col justify-between gap-4 rounded-xl border border-slate-200 p-4 transition-colors hover:border-indigo-300">
              <div>
                <p className="font-medium text-slate-900">{plan.name}</p>
                <p className="mt-0.5 text-2xl font-semibold tracking-tight text-slate-900">
                  {formatMoney(plan.amount)}
                </p>
              </div>
              <Button
                variant="primary"
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
                {startCheckout.isPending ? "Starting…" : "Subscribe"}
              </Button>
            </div>
          </li>
        ))}
      </ul>
      {startCheckout.isError && (
        <Alert>
          Could not start checkout:{" "}
          {startCheckout.error instanceof ApiProblem
            ? startCheckout.error.detail
            : "Something went wrong."}
        </Alert>
      )}
    </div>
  );
}

/** U1, D3's five-way state table. `incomplete` offers cancel directly here
 * (D2's demo path depends on it); `active` renders `PlanSelector` (U2);
 * no-subscription offers a plan choice that starts Checkout. */
export function SubscriptionPanel() {
  const query = useSubscription();
  const cancelSubscription = useCancelSubscription();

  if (query.isPending) {
    return (
      <Card title="Subscription">
        <Loading>Loading your subscription…</Loading>
      </Card>
    );
  }

  if (query.isError) {
    const detail =
      query.error instanceof ApiProblem ? query.error.detail : "Something went wrong.";
    return (
      <Card title="Subscription">
        <Alert>Could not load your subscription: {detail}</Alert>
      </Card>
    );
  }

  const subscription = query.data;

  if (subscription === null) {
    return (
      <Card title="Subscription" subtitle="Pick a plan to start a Stripe Checkout session">
        <div className="space-y-4">
          <EmptyState>You don't have a subscription yet.</EmptyState>
          <StartSubscription />
        </div>
      </Card>
    );
  }

  if (subscription.status === "incomplete") {
    return (
      <Card title="Subscription" action={<Badge tone="amber">incomplete</Badge>}>
        <div className="space-y-4">
          <p className="text-sm text-slate-600">
            Your subscription was created but never paid for.
          </p>
          <Button
            variant="danger"
            disabled={cancelSubscription.isPending}
            onClick={() => cancelSubscription.mutate(subscription.id)}
          >
            {cancelSubscription.isPending ? "Cancelling…" : "Cancel"}
          </Button>
          {cancelSubscription.isError && (
            <Alert>
              Could not cancel:{" "}
              {cancelSubscription.error instanceof ApiProblem
                ? cancelSubscription.error.detail
                : "Something went wrong."}
            </Alert>
          )}
        </div>
      </Card>
    );
  }

  return (
    <Card
      title="Subscription"
      action={<Badge tone={STATUS_TONE[subscription.status]}>{subscription.status}</Badge>}
    >
      <div className="space-y-5">
        <Row>
          <div>
            <p className="text-xs uppercase tracking-wide text-slate-400">Current period</p>
            <p className="mt-0.5 text-sm font-medium text-slate-800">
              {formatDate(subscription.current_period_start)} –{" "}
              {formatDate(subscription.current_period_end)}
            </p>
          </div>
          {subscription.cancel_at_period_end && <Badge tone="amber">cancels at period end</Badge>}
        </Row>
        {subscription.status === "active" && (
          <PlanSelector subscriptionId={subscription.id} currentPlanId={subscription.plan_id} />
        )}
      </div>
    </Card>
  );
}
