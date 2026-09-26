import { useQuery } from "@tanstack/react-query";
import { useAuth } from "../../host/auth/AuthContext";
import { getInvoices } from "../api/endpoints";
import { billingKeys } from "../api/keys";

export function useInvoices(after?: string) {
  const { tenantId } = useAuth();
  return useQuery({
    queryKey: billingKeys.invoices(tenantId, after),
    queryFn: () => getInvoices(after),
  });
}
