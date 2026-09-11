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
import { Alert, Badge, Button, Card, Loading } from "../../ui/primitives";
import type { SubscriptionStatus } from "../api/types";

const STATUS_TONE: Record<SubscriptionStatus, "positive" | "caution" | "negative"> = {
  active: "positive",
  past_due: "caution",
  incomplete: "caution",
  incomplete_expired: "negative",
  canceled: "negative",
};

const SUBTITLE = "Live mirror of this tenant's Stripe subscription";

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
    <div className="stack">
      <div className="plan-grid">
        {plansQuery.data.map((plan) => (
          <div key={plan.id} className="plan-card" style={{ boxShadow: "var(--shadow-sm)" }}>
            <div className="plan-card-head">
              <span className="plan-name">{plan.name}</span>
            </div>
            <div className="plan-price">{formatMoney(plan.amount)}</div>
            <Button
              variant="primary"
              block
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
        ))}
      </div>
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

/** One branch per subscription state. `incomplete` offers cancel directly
 * here (seeded subscriptions start `incomplete`, and the demo needs a way
 * out); `active` renders `PlanSelector`; no-subscription offers a plan
 * choice that starts Checkout. */
export function SubscriptionPanel() {
  const query = useSubscription();
  const cancelSubscription = useCancelSubscription();

  if (query.isPending) {
    return (
      <Card title="Subscription" subtitle={SUBTITLE} prominent>
        <Loading>Loading your subscription…</Loading>
      </Card>
    );
  }

  if (query.isError) {
    const detail =
      query.error instanceof ApiProblem ? query.error.detail : "Something went wrong.";
    return (
      <Card title="Subscription" subtitle={SUBTITLE} prominent>
        <Alert>Could not load your subscription: {detail}</Alert>
      </Card>
    );
  }

  const subscription = query.data;

  if (subscription === null) {
    return (
      <Card title="Subscription" subtitle={SUBTITLE} prominent>
        <div className="stack" style={{ gap: "12px" }}>
          <p style={{ margin: 0, fontSize: "14px" }}>You don't have a subscription yet.</p>
          <span className="panel-sub">Pick a plan to start a Stripe Checkout session</span>
          <StartSubscription />
        </div>
      </Card>
    );
  }

  if (subscription.status === "incomplete") {
    return (
      <Card title="Subscription" subtitle={SUBTITLE} prominent>
        <div className="stack">
          <Badge tone="caution">incomplete</Badge>
          <p style={{ margin: 0, fontSize: "14px" }}>
            Your subscription was created but never paid for.
          </p>
          <div>
            <Button
              variant="destructive"
              disabled={cancelSubscription.isPending}
              onClick={() => cancelSubscription.mutate(subscription.id)}
            >
              {cancelSubscription.isPending ? "Cancelling…" : "Cancel"}
            </Button>
          </div>
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
    <Card title="Subscription" subtitle={SUBTITLE} prominent>
      <div className="stack" style={{ gap: "14px" }}>
        <div style={{ display: "flex", gap: "8px", flexWrap: "wrap" }}>
          <Badge tone={STATUS_TONE[subscription.status]}>{subscription.status}</Badge>
          {subscription.cancel_at_period_end && <Badge tone="caution">cancels at period end</Badge>}
        </div>
        <div>
          <div className="field-label">Current period</div>
          <div style={{ fontSize: "14px" }}>
            {formatDate(subscription.current_period_start)} –{" "}
            {formatDate(subscription.current_period_end)}
          </div>
        </div>
        {subscription.status === "active" && (
          <>
            <div className="hr" />
            <PlanSelector
              subscriptionId={subscription.id}
              currentPlanId={subscription.plan_id}
            />
          </>
        )}
      </div>
    </Card>
  );
}
