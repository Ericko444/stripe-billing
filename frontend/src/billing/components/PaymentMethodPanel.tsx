import { useEffect, useState, type FormEvent } from "react";
import { loadStripe } from "@stripe/stripe-js";
import { Elements, PaymentElement, useElements, useStripe } from "@stripe/react-stripe-js";
import { ApiProblem } from "../../host/api/apiClient";
import { useCreateSetupIntent, usePaymentMethods } from "../hooks/usePaymentMethods";

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
    <form onSubmit={handleSubmit}>
      <PaymentElement />
      <button type="submit" disabled={!stripe || saving}>
        {saving ? "Saving…" : "Save card"}
      </button>
      {error && <p role="alert">{error}</p>}
    </form>
  );
}

/** U4. Only the SetupIntent/Element flow and its pending poll (D4/D5) --
 * set-default and detach are Task 11. */
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
    return <section aria-busy="true">Loading payment methods…</section>;
  }

  if (query.isError) {
    const detail =
      query.error instanceof ApiProblem ? query.error.detail : "Something went wrong.";
    return <section role="alert">Could not load payment methods: {detail}</section>;
  }

  function startAddCard() {
    setPhase({ kind: "creating-intent" });
    createSetupIntent.mutate(undefined, {
      onSuccess: (data) => setPhase({ kind: "form", clientSecret: data.client_secret }),
      onError: () => setPhase({ kind: "idle" }),
    });
  }

  return (
    <section>
      {query.data.length === 0 ? (
        <p>No payment methods yet.</p>
      ) : (
        <ul>
          {query.data.map((pm) => (
            <li key={pm.id}>
              {pm.brand} •••• {pm.last4}
              {pm.is_default ? " (default)" : ""}
            </li>
          ))}
        </ul>
      )}

      {phase.kind === "idle" && (
        <button type="button" onClick={startAddCard}>
          Add a card
        </button>
      )}
      {phase.kind === "creating-intent" && <p aria-busy="true">Loading payment form…</p>}
      {phase.kind === "form" && (
        <Elements stripe={stripePromise} options={{ clientSecret: phase.clientSecret }}>
          <AddCardForm
            onSaved={() => setPhase({ kind: "pending", countBefore: query.data.length })}
          />
        </Elements>
      )}
      {phase.kind === "pending" && <p aria-busy="true">Confirming your card…</p>}
      {phase.kind === "capped" && (
        <p role="alert">Still waiting for Stripe to confirm. Is `stripe listen` running?</p>
      )}
    </section>
  );
}
