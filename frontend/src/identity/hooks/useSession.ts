import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { getMe, login, logout, selectTenant } from "../api/endpoints";
import { identityKeys } from "../api/keys";
import type { LoginRequest } from "../api/types";

/** The current session, or `null` when there is none. Not retried: a
 * refusal is an answer, and a network failure should surface, not spin. */
export function useSession() {
  return useQuery({
    queryKey: identityKeys.session,
    queryFn: getMe,
    retry: false,
    staleTime: Infinity,
  });
}

export function useLogin() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: LoginRequest) => login(body),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: identityKeys.session }),
  });
}

/** Scopes the session to a tenant. The response carries a rotated cookie;
 * clearing every other feature's cached data is the host's job, not this
 * hook's -- it does not know what else is cached. */
export function useSwitchTenant() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (tenantId: string) => selectTenant(tenantId),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: identityKeys.session }),
  });
}

/** Always ends in "logged out" on this side, even if the request failed:
 * the server clears the cookie on every answer it can give. */
export function useLogout() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () => logout(),
    onSettled: () => queryClient.invalidateQueries({ queryKey: identityKeys.session }),
  });
}
