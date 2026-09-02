import { useQuery } from "@tanstack/react-query";
import { useAuth } from "../../host/auth/AuthContext";
import { getSubscription } from "../api/endpoints";
import { billingKeys } from "../api/keys";

export function useSubscription() {
  const { tenantId, token } = useAuth();
  return useQuery({
    queryKey: billingKeys.subscription(tenantId),
    queryFn: getSubscription,
    enabled: token !== null,
  });
}
