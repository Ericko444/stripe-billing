import { useState, type FormEvent, type ReactNode } from "react";
import { ApiProblem } from "../../host/api/apiClient";
import { useLogin } from "../hooks/useSession";
import { Alert, Button } from "../../ui/primitives";

/**
 * Email and password, and nothing clever: the server decides everything and
 * answers every refusal the same way, so this form has one error to show --
 * whatever `detail` the problem carries.
 *
 * Idle, submitting (every input and the button disabled, so a double click
 * is one attempt), refused (the problem's `detail`, inputs editable again).
 * Success needs no state of its own: the session query is invalidated and
 * the shell moves on.
 */
export function LoginForm({ notice }: { notice?: ReactNode }) {
  const login = useLogin();
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    login.mutate(
      { email, password },
      // The password is not kept in component state past a refusal: the user
      // retypes it, the same as every login form that does not echo it back.
      { onError: () => setPassword("") },
    );
  }

  return (
    <form className="card elev-md stack" style={{ padding: "24px", width: "100%", maxWidth: "380px" }} onSubmit={submit}>
      <div>
        <h1 className="panel-title is-prominent">Log in</h1>
        <p className="panel-sub">Use the account your tenant invited you with.</p>
      </div>

      {notice}

      <label className="stack" style={{ gap: "2px" }}>
        <span className="field-label">Email</span>
        <input
          className="input"
          type="email"
          name="email"
          autoComplete="username"
          required
          value={email}
          disabled={login.isPending}
          onChange={(event) => setEmail(event.target.value)}
        />
      </label>
      <label className="stack" style={{ gap: "2px" }}>
        <span className="field-label">Password</span>
        <input
          className="input"
          type="password"
          name="password"
          autoComplete="current-password"
          required
          value={password}
          disabled={login.isPending}
          onChange={(event) => setPassword(event.target.value)}
        />
      </label>

      {login.isError && (
        <Alert>
          {login.error instanceof ApiProblem && login.error.detail
            ? login.error.detail
            : "Could not reach the server. Try again."}
        </Alert>
      )}

      <Button type="submit" variant="primary" block disabled={login.isPending} aria-busy={login.isPending}>
        {login.isPending ? "Logging in…" : "Log in"}
      </Button>
    </form>
  );
}
