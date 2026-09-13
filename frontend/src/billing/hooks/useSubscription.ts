import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useAuth } from "../../host/auth/AuthContext";
import { cancelSubscription, getSubscription, startCheckoutSession } from "../api/endpoints";
import { billingKeys } from "../api/keys";

export function useSubscription() {
  const { tenantId } = useAuth();
  return useQuery({
    queryKey: billingKeys.subscription(tenantId),
    queryFn: getSubscription,
  });
}

export function useCancelSubscription() {
  const { tenantId } = useAuth();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (subscriptionId: string) =>
      cancelSubscription(subscriptionId, { at_period_end: false }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: billingKeys.subscription(tenantId) });
    },
  });
}

/** No `onSuccess` invalidation here -- the caller redirects the browser to
 * `data.url` (Stripe's hosted page) before any refetch would matter. The
 * subscription this creates does not exist locally until the webhook
 * mirrors it, which `CheckoutReturn`'s pending poll waits out. */
export function useStartCheckoutSession() {
  return useMutation({
    mutationFn: (planId: string) => startCheckoutSession({ plan_id: planId }),
  });
}
