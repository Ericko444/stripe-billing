import { apiRequest, ApiProblem } from "../../host/api/apiClient";
import type { LoginRequest, MeDto, SessionDto } from "./types";

/**
 * One function per identity route this feature calls.
 */

export function login(body: LoginRequest): Promise<SessionDto> {
  // A refused login is a 401, but it is not "the session ended" -- there was
  // no session. Keep it out of the host's unauthorized handler.
  return apiRequest("/auth/login", { method: "POST", body: JSON.stringify(body) }, {
    reportUnauthorized: false,
  });
}

/**
 * The only way the app learns whether it is logged in: the cookie is
 * invisible to script. `null` means "no session", which is a normal state,
 * not an error -- so a 401 here resolves rather than throws, and is not
 * reported to the host's unauthorized handler (it would loop).
 */
export async function getMe(): Promise<MeDto | null> {
  try {
    return await apiRequest<MeDto>("/auth/me", undefined, { reportUnauthorized: false });
  } catch (error) {
    if (error instanceof ApiProblem && error.status === 401) {
      return null;
    }
    throw error;
  }
}

export function selectTenant(tenantId: string): Promise<SessionDto> {
  return apiRequest("/auth/tenant", {
    method: "POST",
    body: JSON.stringify({ tenant_id: tenantId }),
  });
}

export function logout(): Promise<void> {
  return apiRequest("/auth/logout", { method: "POST" }, { reportUnauthorized: false });
}
