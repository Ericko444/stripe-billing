import { useState, type FormEvent } from "react";
import { ApiProblem } from "../../host/api/apiClient";
import { useRequestReset } from "../hooks/usePasswordReset";
import { Alert, Button } from "../../ui/primitives";

/** What the page says after a request is accepted -- the same words for
 * every address, like the server's own answer. */
const SENT =
  "If an account exists for that address, a link to reset its password is on its way. It works for 15 minutes.";

/**
 * Ask for a reset link. The answer is the same whatever the address, so this
 * form has one success state and never says whether the address is known.
 *
 * Idle, submitting (disabled), sent (the fixed confirmation), refused (only
 * for a malformed address or too many requests -- neither says anything about
 * an account).
 */
export function ForgotPasswordForm({ onBack }: { onBack: () => void }) {
  const request = useRequestReset();
  const [email, setEmail] = useState("");

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    request.mutate(email);
  }

  if (request.isSuccess) {
    return (
      <div className="card elev-md stack" style={{ padding: "24px", width: "100%", maxWidth: "380px" }}>
        <h1 className="panel-title is-prominent">Check your mail</h1>
        <p role="status">{SENT}</p>
        <Button variant="ghost" size="sm" onClick={onBack}>
          Back to log in
        </Button>
      </div>
    );
  }

  return (
    <form
      className="card elev-md stack"
      style={{ padding: "24px", width: "100%", maxWidth: "380px" }}
      onSubmit={submit}
    >
      <div>
        <h1 className="panel-title is-prominent">Reset your password</h1>
        <p className="panel-sub">We will mail a link to the address, if it has an account.</p>
      </div>

      <label className="stack" style={{ gap: "2px" }}>
        <span className="field-label">Email</span>
        <input
          className="input"
          type="email"
          name="email"
          autoComplete="username"
          required
          value={email}
          disabled={request.isPending}
          onChange={(event) => setEmail(event.target.value)}
        />
      </label>

      {request.isError && (
        <Alert>
          {request.error instanceof ApiProblem && request.error.detail
            ? request.error.detail
            : "Could not reach the server. Try again."}
        </Alert>
      )}

      <Button type="submit" variant="primary" block disabled={request.isPending} aria-busy={request.isPending}>
        {request.isPending ? "Sending…" : "Send the link"}
      </Button>
      <Button type="button" variant="ghost" size="sm" onClick={onBack} disabled={request.isPending}>
        Back to log in
      </Button>
    </form>
  );
}
