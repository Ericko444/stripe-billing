import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useAuth } from "../../host/auth/AuthContext";
import { changePlan, getPlans } from "../api/endpoints";
import { billingKeys } from "../api/keys";

export function usePlans() {
  const { tenantId, token } = useAuth();
  return useQuery({
    queryKey: billingKeys.plans(tenantId),
    queryFn: getPlans,
    enabled: token !== null,
  });
}

export function useChangePlan(subscriptionId: string) {
  const { tenantId } = useAuth();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (planId: string) => changePlan(subscriptionId, { plan_id: planId }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: billingKeys.subscription(tenantId) });
    },
  });
}
