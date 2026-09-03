import { ApiProblem, toApiProblem } from "./problem";

/**
 * The only `fetch` in the app (F1). Every other module goes through
 * `apiRequest`.
 *
 * The token is read via a getter, not a value captured at import time --
 * `AuthContext` calls `setTokenGetter` whenever the tenant switches, and
 * every subsequent request picks up the new token without this module
 * needing to know that switching happened.
 */
let getToken: () => string | null = () => null;
let onUnauthorized: () => void = () => {};

export function setTokenGetter(getter: () => string | null): void {
  getToken = getter;
}

export function setUnauthorizedHandler(handler: () => void): void {
  onUnauthorized = handler;
}

export async function apiRequest<T>(path: string, init?: RequestInit): Promise<T> {
  const token = getToken();
  const headers = new Headers(init?.headers);
  headers.set("Accept", "application/json");
  if (init?.body !== undefined) {
    headers.set("Content-Type", "application/json");
  }
  if (token !== null) {
    headers.set("Authorization", `Bearer ${token}`);
  }

  const response = await fetch(`/api/v1${path}`, { ...init, headers });

  if (response.status === 401) {
    onUnauthorized();
    throw await toApiProblem(response);
  }
  if (!response.ok) {
    throw await toApiProblem(response);
  }
  if (response.status === 204) {
    return undefined as T;
  }
  return (await response.json()) as T;
}

export { ApiProblem };
