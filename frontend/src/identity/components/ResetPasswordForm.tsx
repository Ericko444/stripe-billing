import { useEffect, useState, type FormEvent } from "react";
import { ApiProblem } from "../../host/api/apiClient";
import { useCompleteReset } from "../hooks/usePasswordReset";
import { MAX_PASSWORD_CHARS, MIN_PASSWORD_CHARS, passwordProblem } from "../password";
import { Alert, Button } from "../../ui/primitives";

/**
 * The token from the URL **fragment** -- `#token=...` -- and nowhere else.
 * A fragment never reaches a server, a proxy log or a `Referer`; a query
 * string would reach all three. `location.search` is deliberately not read.
 */
export function readTokenFromFragment(hash: string): string | null {
  const params = new URLSearchParams(hash.startsWith("#") ? hash.slice(1) : hash);
  const token = params.get("token");
  return token && token.length > 0 ? token : null;
}

/**
 * Set a new password through the mailed link.
 *
 * The token is read once, on first render, and then removed from the address
 * bar with `history.replaceState` -- so it is not left in the browser history,
 * a bookmark, or a screenshot. It lives only in this component's state.
 *
 * Idle, submitting (disabled), done (log in with the new password -- no
 * session is started for you), refused: a password the policy refuses stays
 * on the form with the reason; a link that cannot be used switches to one
 * "invalid or expired" state, whatever the cause, with a way to ask again.
 */
export function ResetPasswordForm({
  onRequestNewLink,
  onDone,
}: {
  onRequestNewLink: () => void;
  onDone: () => void;
}) {
  const [token] = useState(() => readTokenFromFragment(window.location.hash));
  const complete = useCompleteReset();
  const [password, setPassword] = useState("");
  const [confirmation, setConfirmation] = useState("");
  const [localProblem, setLocalProblem] = useState<string | null>(null);

  useEffect(() => {
    if (window.location.hash) {
      window.history.replaceState(null, "", window.location.pathname);
    }
  }, []);

  const linkUnusable =
    token === null ||
    (complete.error instanceof ApiProblem && complete.error.status === 400);

  if (linkUnusable) {
    return (
      <div
        className="card elev-md stack"
        style={{ padding: "24px", width: "100%", maxWidth: "380px" }}
        role="alert"
      >
        <h1 className="panel-title is-prominent">This link can't be used</h1>
        <p>It is invalid, has expired, or was already used. Reset links work once, for 15 minutes.</p>
        <Button variant="primary" onClick={onRequestNewLink}>
          Request a new link
        </Button>
      </div>
    );
  }

  if (complete.isSuccess) {
    return (
      <div className="card elev-md stack" style={{ padding: "24px", width: "100%", maxWidth: "380px" }}>
        <h1 className="panel-title is-prominent">Password changed</h1>
        <p role="status">
          Every session on this account has been ended. Log in with your new password.
        </p>
        <Button variant="primary" onClick={onDone}>
          Go to log in
        </Button>
      </div>
    );
  }

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const problem = passwordProblem(password, confirmation);
    setLocalProblem(problem);
    if (problem === null && token !== null) {
      complete.mutate({ token, newPassword: password });
    }
  }

  const serverProblem =
    complete.isError && !(complete.error instanceof ApiProblem && complete.error.status === 400)
      ? complete.error instanceof ApiProblem && complete.error.detail
        ? complete.error.detail
        : "Could not reach the server. Try again."
      : null;

  return (
    <form
      className="card elev-md stack"
      style={{ padding: "24px", width: "100%", maxWidth: "380px" }}
      onSubmit={submit}
    >
      <div>
        <h1 className="panel-title is-prominent">Choose a new password</h1>
        <p className="panel-sub">
          {MIN_PASSWORD_CHARS} to {MAX_PASSWORD_CHARS} characters. A passphrase is easiest.
        </p>
      </div>

      <label className="stack" style={{ gap: "2px" }}>
        <span className="field-label">New password</span>
        <input
          className="input"
          type="password"
          name="new-password"
          autoComplete="new-password"
          required
          value={password}
          disabled={complete.isPending}
          onChange={(event) => setPassword(event.target.value)}
        />
      </label>
      <label className="stack" style={{ gap: "2px" }}>
        <span className="field-label">Repeat it</span>
        <input
          className="input"
          type="password"
          name="confirm-password"
          autoComplete="new-password"
          required
          value={confirmation}
          disabled={complete.isPending}
          onChange={(event) => setConfirmation(event.target.value)}
        />
      </label>

      {(localProblem ?? serverProblem) && <Alert>{localProblem ?? serverProblem}</Alert>}

      <Button type="submit" variant="primary" block disabled={complete.isPending} aria-busy={complete.isPending}>
        {complete.isPending ? "Saving…" : "Set the new password"}
      </Button>
    </form>
  );
}
