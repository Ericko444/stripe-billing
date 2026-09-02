import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useAuth } from "../../host/auth/AuthContext";
import {
  createSetupIntent,
  getPaymentMethods,
  removePaymentMethod,
  setDefaultPaymentMethod,
} from "../api/endpoints";
import { billingKeys } from "../api/keys";

export function usePaymentMethods() {
  const { tenantId, token } = useAuth();
  return useQuery({
    queryKey: billingKeys.paymentMethods(tenantId),
    queryFn: getPaymentMethods,
    enabled: token !== null,
  });
}

/** No invalidation here -- the `client_secret` this returns is confirmed
 * client-side against Stripe.js, and the card does not exist in this API
 * until the `payment_method.attached` / `setup_intent.succeeded` webhook
 * mirrors it. `PaymentMethodPanel`'s own pending poll (D5) is what notices. */
export function useCreateSetupIntent() {
  return useMutation({
    mutationFn: createSetupIntent,
  });
}

export function useSetDefaultPaymentMethod() {
  const { tenantId } = useAuth();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: setDefaultPaymentMethod,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: billingKeys.paymentMethods(tenantId) });
    },
  });
}

export function useRemovePaymentMethod() {
  const { tenantId } = useAuth();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: removePaymentMethod,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: billingKeys.paymentMethods(tenantId) });
    },
  });
}
