import { createContext, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { setUnauthorizedHandler } from "../api/apiClient";
import { identityKeys } from "../../identity/api/keys";
import type { MeDto, MembershipDto, Role } from "../../identity/api/types";
import { useLogout, useSession, useSwitchTenant } from "../../identity/hooks/useSession";

/**
 * The host's view of the session, built on the identity feature and handed
 * to everything else. Billing reads one thing from it -- `tenantId` -- and
 * only through `useAuth()`, which exists only while a tenant is picked.
 */
export type SessionStatus =
  /** The first `GET /auth/me` has not answered. */
  | "loading"
  /** No session. */
  | "anonymous"
  /** Logged in, several memberships, none picked yet. */
  | "choosingTenant"
  /** Logged in and scoped to a tenant. */
  | "ready"
  /** The session probe itself failed -- the API is unreachable. */
  | "unavailable";

interface SessionContextValue {
  status: SessionStatus;
  me: MeDto | null;
  /** True after a session that existed stopped being accepted -- expired,
   * revoked, suspended. Not set on first load, and not set by logging out. */
  sessionEnded: boolean;
  problem: Error | null;
  switchTenant: (tenantId: string) => void;
  isSwitching: boolean;
  switchProblem: Error | null;
  logout: () => void;
  isLoggingOut: boolean;
  retry: () => void;
}

const SessionContext = createContext<SessionContextValue | null>(null);

function statusOf(isPending: boolean, isError: boolean, me: MeDto | null | undefined): SessionStatus {
  if (isPending) return "loading";
  if (isError) return "unavailable";
  if (!me) return "anonymous";
  return me.tenant === null ? "choosingTenant" : "ready";
}

export function AuthProvider({ children }: { children: ReactNode }) {
  const queryClient = useQueryClient();
  const session = useSession();
  const switchMutation = useSwitchTenant();
  const logoutMutation = useLogout();

  const me = session.data ?? null;
  const status = statusOf(session.isPending, session.isError, session.data);

  // "Session ended" means a session we had stopped being accepted without the
  // user asking. Logging out is the user asking, so it is excluded.
  const hadSession = useRef(false);
  const loggingOut = useRef(false);
  const [sessionEnded, setSessionEnded] = useState(false);
  useEffect(() => {
    if (me) {
      hadSession.current = true;
      setSessionEnded(false);
    } else if (session.isSuccess && hadSession.current) {
      hadSession.current = false;
      setSessionEnded(!loggingOut.current);
      loggingOut.current = false;
    }
  }, [me, session.isSuccess]);

  // A 401 from any feature's request means the session may be gone: re-ask.
  useEffect(() => {
    setUnauthorizedHandler(() => {
      void queryClient.invalidateQueries({ queryKey: identityKeys.session });
    });
  }, [queryClient]);

  // Cache partitioning, not authorization: every billing key already carries
  // the tenant id, so this is belt-and-braces -- but a switch or a logout
  // must never leave the previous tenant's data one render away.
  function dropEverythingButTheSession() {
    queryClient.removeQueries({ predicate: (query) => query.queryKey[0] !== identityKeys.all[0] });
  }

  function switchTenant(tenantId: string) {
    if (me?.tenant?.tenant_id === tenantId) {
      return;
    }
    switchMutation.mutate(tenantId, { onSuccess: dropEverythingButTheSession });
  }

  function logout() {
    loggingOut.current = true;
    logoutMutation.mutate(undefined, { onSettled: dropEverythingButTheSession });
  }

  const value: SessionContextValue = {
    status,
    me,
    sessionEnded,
    problem: session.error,
    switchTenant,
    isSwitching: switchMutation.isPending,
    switchProblem: switchMutation.error,
    logout,
    isLoggingOut: logoutMutation.isPending,
    retry: () => void session.refetch(),
  };

  return <SessionContext.Provider value={value}>{children}</SessionContext.Provider>;
}

/** The session, in whatever state it is -- for the app shell. */
export function useSessionState(): SessionContextValue {
  const ctx = useContext(SessionContext);
  if (ctx === null) {
    throw new Error("useSessionState must be used within an AuthProvider");
  }
  return ctx;
}

export interface AuthContextValue {
  tenantId: string;
  tenantName: string;
  role: Role;
  memberships: MembershipDto[];
  switchTenant: (tenantId: string) => void;
  logout: () => void;
}

/**
 * The tenant-scoped session. Only callable where a tenant is picked -- the
 * shell renders billing only in that branch -- so `tenantId` is a `string`,
 * never `null`, and no billing hook needs to guard on it.
 */
export function useAuth(): AuthContextValue {
  const { me, switchTenant, logout } = useSessionState();
  if (me === null || me.tenant === null) {
    throw new Error("useAuth must be used within a tenant-scoped session");
  }
  return {
    tenantId: me.tenant.tenant_id,
    tenantName: me.tenant.tenant_name,
    role: me.tenant.role,
    memberships: me.memberships,
    switchTenant,
    logout,
  };
}
