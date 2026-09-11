import { createContext, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { setTokenGetter, setUnauthorizedHandler } from "../api/apiClient";

/** The two tenants `cargo run -p demo -- seed` creates -- fixed ids, not
 * random, per `crates/demo/src/seed.rs`. */
export const SEEDED_TENANTS = {
  "00000000-0000-0000-0000-000000000001": "Tenant A",
  "00000000-0000-0000-0000-000000000002": "Tenant B",
} as const;

export type TenantId = keyof typeof SEEDED_TENANTS;

const DEFAULT_TENANT: TenantId = "00000000-0000-0000-0000-000000000001";

/** Checkout is a full-page redirect to Stripe and back -- the SPA remounts
 * from scratch on return, so the in-memory tenant selection (the token
 * itself is never persisted) would otherwise silently reset to
 * `DEFAULT_TENANT` even if a different tenant started the session. Not a
 * credential, so `sessionStorage` for this one value doesn't undermine the
 * never-persist-the-token rule -- it's which tenant to resume as, not proof
 * of who they are. */
const RETURN_TENANT_KEY = "billing-demo:return-tenant";

function isSeededTenant(value: string): value is TenantId {
  return value in SEEDED_TENANTS;
}

/** Called before redirecting to Stripe, so the tenant survives the round
 * trip through `CheckoutReturn`. */
export function rememberTenantForReturn(tenantId: TenantId): void {
  sessionStorage.setItem(RETURN_TENANT_KEY, tenantId);
}

function initialTenant(): TenantId {
  const stored = sessionStorage.getItem(RETURN_TENANT_KEY);
  if (stored !== null) {
    sessionStorage.removeItem(RETURN_TENANT_KEY);
    if (isSeededTenant(stored)) {
      return stored;
    }
  }
  return DEFAULT_TENANT;
}

interface MintTokenResponse {
  token: string;
  expires_at: string;
}

interface AuthState {
  tenantId: TenantId;
  token: string | null;
  isMinting: boolean;
  error: string | null;
}

interface AuthContextValue extends AuthState {
  switchTenant: (tenantId: TenantId) => void;
}

const AuthContext = createContext<AuthContextValue | null>(null);

/** Mints a token directly against `fetch` rather than `apiRequest` --
 * `/demo/token` takes no `Authorization` header, and routing it through the
 * same helper that injects one would be backwards for the one call that
 * produces the token in the first place. */
async function mintToken(tenantId: TenantId): Promise<MintTokenResponse> {
  const response = await fetch("/api/v1/demo/token", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ tenant_id: tenantId }),
  });
  if (!response.ok) {
    throw new Error(`token mint failed: ${response.status}`);
  }
  return (await response.json()) as MintTokenResponse;
}

export function AuthProvider({ children }: { children: ReactNode }) {
  const queryClient = useQueryClient();
  const [state, setState] = useState<AuthState>(() => ({
    tenantId: initialTenant(),
    token: null,
    isMinting: true,
    error: null,
  }));

  // `tokenRef` is kept current synchronously during render, not in an
  // effect: a descendant's query can fire in the same commit that flips
  // `token` from null to a real value, before an effect keyed on
  // `state.token` would have re-run -- registering the getter once, over a
  // ref instead of the state value, closes that race rather than narrowing
  // it.
  const tokenRef = useRef<string | null>(state.token);
  tokenRef.current = state.token;

  useEffect(() => {
    setTokenGetter(() => tokenRef.current);
  }, []);

  useEffect(() => {
    setUnauthorizedHandler(() => {
      setState((prev) => ({ ...prev, token: null, error: "Session expired." }));
    });
  }, []);

  useEffect(() => {
    let cancelled = false;
    setState((prev) => ({ ...prev, isMinting: true, error: null }));
    mintToken(state.tenantId)
      .then(({ token }) => {
        if (!cancelled) {
          setState((prev) => ({ ...prev, token, isMinting: false }));
        }
      })
      .catch((err: unknown) => {
        if (!cancelled) {
          setState((prev) => ({
            ...prev,
            isMinting: false,
            error: err instanceof Error ? err.message : "Could not mint a demo token.",
          }));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [state.tenantId]);

  function switchTenant(tenantId: TenantId): void {
    if (tenantId === state.tenantId) {
      return;
    }
    // Cache partitioning, not authorization -- every query key already
    // carries tenantId, so this `clear()` is belt-and-suspenders against a
    // component that forgot to.
    queryClient.clear();
    setState((prev) => ({ ...prev, tenantId, token: null }));
  }

  return (
    <AuthContext.Provider value={{ ...state, switchTenant }}>{children}</AuthContext.Provider>
  );
}

export function useAuth(): AuthContextValue {
  const ctx = useContext(AuthContext);
  if (ctx === null) {
    throw new Error("useAuth must be used within an AuthProvider");
  }
  return ctx;
}
