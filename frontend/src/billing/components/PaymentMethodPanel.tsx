import { useEffect, useState, type FormEvent } from "react";
import { loadStripe } from "@stripe/stripe-js";
import { Elements, PaymentElement, useElements, useStripe } from "@stripe/react-stripe-js";
import { ApiProblem } from "../../host/api/apiClient";
import {
  useCreateSetupIntent,
  usePaymentMethods,
  useRemovePaymentMethod,
  useSetDefaultPaymentMethod,
} from "../hooks/usePaymentMethods";
import type { PaymentMethodDto } from "../api/types";
import { Alert, Badge, Button, Card, EmptyState, Loading, Notice, Row } from "../../ui/primitives";

// F3: only the publishable key crosses into the bundle -- the VITE_ prefix
// is what Vite inlines, and nothing carrying a secret may use it.
const PUBLISHABLE_KEY: string = import.meta.env.VITE_STRIPE_PUBLISHABLE_KEY;
if (!PUBLISHABLE_KEY) {
  throw new Error("VITE_STRIPE_PUBLISHABLE_KEY is not set");
}
const stripePromise = loadStripe(PUBLISHABLE_KEY);

const POLL_INTERVAL_MS = 2000;
const MAX_ATTEMPTS = 10;

type AddCardPhase =
  | { kind: "idle" }
  | { kind: "creating-intent" }
  | { kind: "form"; clientSecret: string }
  | { kind: "pending"; countBefore: number }
  | { kind: "capped" };

/** D4: `redirect: 'if_required'` -- cards confirm inline, no return route
 * needed. `onSaved` hands control back to the panel, which owns the
 * pending poll (the card does not exist in this API until the webhook
 * mirrors it, same reasoning as `CheckoutReturn`). */
function AddCardForm({ onSaved }: { onSaved: () => void }) {
  const stripe = useStripe();
  const elements = useElements();
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!stripe || !elements) {
      return;
    }
    setSaving(true);
    setError(null);
    const { error: confirmError } = await stripe.confirmSetup({
      elements,
      redirect: "if_required",
    });
    setSaving(false);
    if (confirmError) {
      setError(confirmError.message ?? "Could not save the card.");
      return;
    }
    onSaved();
  }

  return (
    <form
      onSubmit={handleSubmit}
      className="space-y-4 rounded-xl border border-slate-200 bg-slate-50/60 p-4"
    >
      <PaymentElement />
      <Button type="submit" variant="primary" disabled={!stripe || saving}>
        {saving ? "Saving…" : "Save card"}
      </Button>
      {error && <Alert>{error}</Alert>}
    </form>
  );
}

/** One card row: set-default and detach, the latter gated by an in-page
 * confirm step rather than `window.confirm` (Task 11). */
function PaymentMethodRow({ pm }: { pm: PaymentMethodDto }) {
  const setDefault = useSetDefaultPaymentMethod();
  const removeMethod = useRemovePaymentMethod();
  const [confirmingRemove, setConfirmingRemove] = useState(false);

  return (
    <li>
      <Row>
        <div className="flex items-center gap-3">
          <span className="flex h-8 w-12 items-center justify-center rounded-md border border-slate-200 bg-slate-50 text-[10px] font-bold uppercase tracking-wide text-slate-600">
            {pm.brand}
          </span>
          <span className="font-medium tabular-nums text-slate-900">•••• {pm.last4}</span>
          {pm.is_default && <Badge tone="green">default</Badge>}
        </div>

        <div className="ml-auto flex items-center gap-1">
          {!pm.is_default && (
            <Button
              variant="ghost"
              disabled={setDefault.isPending}
              onClick={() => setDefault.mutate(pm.id)}
            >
              Make default
            </Button>
          )}
          {!confirmingRemove && (
            <Button variant="ghost" onClick={() => setConfirmingRemove(true)}>
              Remove
            </Button>
          )}
        </div>

        {/* Its own full-width line inside the wrapping row: in the narrow
            column the gate would otherwise reflow the card identity. */}
        {confirmingRemove && (
          <div className="flex w-full flex-wrap items-center justify-between gap-2 rounded-lg bg-red-50 px-3 py-2">
            <span className="text-sm font-medium text-red-800">Remove this card?</span>
            <span className="flex items-center gap-1">
              <Button
                variant="danger"
                disabled={removeMethod.isPending}
                onClick={() =>
                  removeMethod.mutate(pm.id, { onSettled: () => setConfirmingRemove(false) })
                }
              >
                Confirm
              </Button>
              <Button variant="ghost" onClick={() => setConfirmingRemove(false)}>
                Cancel
              </Button>
            </span>
          </div>
        )}

        {setDefault.isError && (
          <span role="alert" className="text-sm text-red-700">
            Could not set default.
          </span>
        )}
        {removeMethod.isError && (
          <span role="alert" className="text-sm text-red-700">
            Could not remove.
          </span>
        )}
      </Row>
    </li>
  );
}

/** U4. */
export function PaymentMethodPanel() {
  const query = usePaymentMethods();
  const createSetupIntent = useCreateSetupIntent();
  const [phase, setPhase] = useState<AddCardPhase>({ kind: "idle" });
  const [attempts, setAttempts] = useState(0);

  useEffect(() => {
    if (phase.kind !== "pending") {
      return;
    }
    const arrived = (query.data?.length ?? phase.countBefore) > phase.countBefore;
    if (arrived) {
      setPhase({ kind: "idle" });
      setAttempts(0);
      return;
    }
    if (attempts >= MAX_ATTEMPTS) {
      setPhase({ kind: "capped" });
      return;
    }
    const timer = setTimeout(() => {
      setAttempts((n) => n + 1);
      void query.refetch();
    }, POLL_INTERVAL_MS);
    return () => clearTimeout(timer);
  }, [phase, attempts, query.data]);

  if (query.isPending) {
    return (
      <Card title="Payment methods">
        <Loading>Loading payment methods…</Loading>
      </Card>
    );
  }

  if (query.isError) {
    const detail =
      query.error instanceof ApiProblem ? query.error.detail : "Something went wrong.";
    return (
      <Card title="Payment methods">
        <Alert>Could not load payment methods: {detail}</Alert>
      </Card>
    );
  }

  function startAddCard() {
    setPhase({ kind: "creating-intent" });
    createSetupIntent.mutate(undefined, {
      onSuccess: (data) => setPhase({ kind: "form", clientSecret: data.client_secret }),
      onError: () => setPhase({ kind: "idle" }),
    });
  }

  return (
    <Card
      title="Payment methods"
      action={
        phase.kind === "idle" ? (
          <Button variant="secondary" onClick={startAddCard}>
            Add a card
          </Button>
        ) : undefined
      }
    >
      <div className="space-y-4">
        {query.data.length === 0 ? (
          <EmptyState>No payment methods yet.</EmptyState>
        ) : (
          <ul className="space-y-2">
            {query.data.map((pm) => (
              <PaymentMethodRow key={pm.id} pm={pm} />
            ))}
          </ul>
        )}

        {phase.kind === "creating-intent" && <Loading>Loading payment form…</Loading>}
        {/* F2: the setup-intent call has its own error surface. Without it a
            failed mutation resets `phase` to idle and the button silently
            does nothing, which is the one outcome a demo cannot explain. */}
        {phase.kind === "idle" && createSetupIntent.isError && (
          <Alert>
            Could not start adding a card:{" "}
            {createSetupIntent.error instanceof ApiProblem
              ? createSetupIntent.error.detail
              : "Something went wrong."}
          </Alert>
        )}
        {/* `locale` pinned for the same reason D10 pins `formatMoney`'s: left
            to the browser, the Element renders in whatever language the
            machine happens to be set to, and the demo is in English.
            Predictability beats politeness for something shown live. */}
        {phase.kind === "form" && (
          <Elements
            stripe={stripePromise}
            options={{ clientSecret: phase.clientSecret, locale: "en" }}
          >
            <AddCardForm
              onSaved={() => setPhase({ kind: "pending", countBefore: query.data.length })}
            />
          </Elements>
        )}
        {phase.kind === "pending" && <Notice busy>Confirming your card…</Notice>}
        {phase.kind === "capped" && (
          <Notice>
            Still waiting for Stripe to confirm. Is <code>stripe listen</code> running?
          </Notice>
        )}
      </div>
    </Card>
  );
}
