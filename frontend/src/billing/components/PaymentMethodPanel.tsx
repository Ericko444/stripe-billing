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
import { Alert, Badge, Button, Card, EmptyState, Loading, Notice, Spinner } from "../../ui/primitives";

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
function AddCardForm({ onSaved, onCancel }: { onSaved: () => void; onCancel: () => void }) {
  const stripe = useStripe();
  const elements = useElements();
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // The Element can fail to mount for reasons the API call never sees --
  // most often a `client_secret` for a SetupIntent Stripe already
  // settled (an idempotent replay of a spent key). Without this the
  // form is a blank box with a dead Save button, F2's worst case.
  const [elementFailed, setElementFailed] = useState(false);

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
    <form onSubmit={handleSubmit} className="pe-box">
      {elementFailed ? (
        <Alert>
          The card form could not load. This usually means the setup session was already
          used -- try again in a moment, or reseed the demo database.
        </Alert>
      ) : (
        <PaymentElement
          onLoadError={() => {
            setError(null);
            setElementFailed(true);
          }}
        />
      )}
      <div className="pe-actions">
        {!elementFailed && (
          <Button type="submit" variant="primary" disabled={!stripe || saving}>
            {saving ? "Saving…" : "Save card"}
          </Button>
        )}
        <Button type="button" variant="secondary" onClick={onCancel}>
          Cancel
        </Button>
      </div>
      {error && (
        <div style={{ marginTop: "10px" }}>
          <Alert>{error}</Alert>
        </div>
      )}
    </form>
  );
}

/** One card row: set-default and detach, the latter gated by an in-page
 * confirm step rather than `window.confirm` (Task 11). */
function PaymentMethodRow({ pm }: { pm: PaymentMethodDto }) {
  const setDefault = useSetDefaultPaymentMethod();
  const removeMethod = useRemovePaymentMethod();
  const [confirmingRemove, setConfirmingRemove] = useState(false);

  if (confirmingRemove) {
    return (
      <li className="row">
        <span>Remove this card?</span>
        <span className="row-actions">
          <Button
            variant="destructive"
            size="sm"
            disabled={removeMethod.isPending}
            onClick={() =>
              removeMethod.mutate(pm.id, { onSettled: () => setConfirmingRemove(false) })
            }
          >
            Confirm
          </Button>
          <Button variant="secondary" size="sm" onClick={() => setConfirmingRemove(false)}>
            Cancel
          </Button>
        </span>
      </li>
    );
  }

  return (
    <li className="row">
      <span className="row-main">
        <Badge tone="neutral">{pm.brand}</Badge>
        <span>•••• {pm.last4}</span>
        {pm.is_default && <Badge tone="neutral">default</Badge>}
      </span>
      <span className="row-actions">
        {!pm.is_default && (
          <Button
            variant="ghost"
            size="sm"
            disabled={setDefault.isPending}
            onClick={() => setDefault.mutate(pm.id)}
          >
            Make default
          </Button>
        )}
        <Button variant="ghost" size="sm" onClick={() => setConfirmingRemove(true)}>
          Remove
        </Button>
      </span>
      {setDefault.isError && (
        <span role="alert" style={{ fontSize: "12px", color: "var(--color-negative)" }}>
          Could not set default.
        </span>
      )}
      {removeMethod.isError && (
        <span role="alert" style={{ fontSize: "12px", color: "var(--color-negative)" }}>
          Could not remove.
        </span>
      )}
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
          <Button variant="ghost" onClick={startAddCard}>
            Add a card
          </Button>
        ) : undefined
      }
    >
      <div className="stack">
        {query.data.length === 0 ? (
          <EmptyState>No payment methods yet.</EmptyState>
        ) : (
          <ul style={{ listStyle: "none", margin: 0, padding: 0 }}>
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
              onCancel={() => setPhase({ kind: "idle" })}
            />
          </Elements>
        )}
        {phase.kind === "pending" && (
          <p aria-busy="true" className="pending-line">
            <Spinner size={15} />
            <span>Confirming your card…</span>
          </p>
        )}
        {phase.kind === "capped" && (
          <Notice>
            Still waiting for Stripe to confirm. Is <code>stripe listen</code> running?
          </Notice>
        )}
      </div>
    </Card>
  );
}
