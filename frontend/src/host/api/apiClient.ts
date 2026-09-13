import { ApiProblem, toApiProblem } from "./problem";

/**
 * The only `fetch` in the app. Every other module goes through
 * `apiRequest`.
 *
 * There is no token here any more. The session is an `HttpOnly` cookie set
 * by the identity module; the browser attaches it to these same-origin
 * requests itself, and script can neither read it nor leak it.
 */
let onUnauthorized: () => void = () => {};

/** Called on a 401 from any request that reports one -- the host uses it to
 * re-check the session, which turns an expired session into the login
 * screen wherever it was noticed. */
export function setUnauthorizedHandler(handler: () => void): void {
  onUnauthorized = handler;
}

export interface RequestOptions {
  /** Whether a 401 means "the session ended" and should reach the handler.
   * Off for the calls where a 401 is an expected answer: the session probe
   * itself, login, logout. Default on. */
  reportUnauthorized?: boolean;
}

export async function apiRequest<T>(
  path: string,
  init?: RequestInit,
  options?: RequestOptions,
): Promise<T> {
  const headers = new Headers(init?.headers);
  headers.set("Accept", "application/json");
  if (init?.body !== undefined) {
    headers.set("Content-Type", "application/json");
  }

  const response = await fetch(`/api/v1${path}`, {
    ...init,
    headers,
    credentials: "same-origin",
  });

  if (!response.ok) {
    if (response.status === 401 && (options?.reportUnauthorized ?? true)) {
      onUnauthorized();
    }
    throw await toApiProblem(response);
  }
  if (response.status === 204) {
    return undefined as T;
  }
  return (await response.json()) as T;
}

export { ApiProblem };
