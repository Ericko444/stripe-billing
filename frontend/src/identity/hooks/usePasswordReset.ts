import { useMutation } from "@tanstack/react-query";
import { completePasswordReset, requestPasswordReset } from "../api/endpoints";

/** No invalidation: requesting a link changes nothing this app has cached. */
export function useRequestReset() {
  return useMutation({
    mutationFn: (email: string) => requestPasswordReset(email),
  });
}

/** No invalidation either: a completed reset starts no session, and the
 * visitor was not logged in to begin with. */
export function useCompleteReset() {
  return useMutation({
    mutationFn: ({ token, newPassword }: { token: string; newPassword: string }) =>
      completePasswordReset(token, newPassword),
  });
}
