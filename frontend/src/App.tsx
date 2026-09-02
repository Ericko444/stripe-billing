import { useQuery } from "@tanstack/react-query";
import { TenantSwitcher } from "./host/auth/TenantSwitcher";
import { useAuth } from "./host/auth/AuthContext";
import { apiRequest } from "./host/api/apiClient";

/** Temporary proof for Task 4: raw JSON of `GET /subscription`, replaced by
 * `SubscriptionPanel` in Task 6. */
function RawSubscriptionProbe() {
  const { tenantId, token } = useAuth();
  const query = useQuery({
    queryKey: ["subscription", tenantId],
    queryFn: () => apiRequest<unknown>("/subscription"),
    enabled: token !== null,
  });

  if (query.isPending) return <p>loading...</p>;
  if (query.isError) return <p>error: {String(query.error)}</p>;
  return <pre>{JSON.stringify(query.data, null, 2)}</pre>;
}

export default function App() {
  return (
    <main>
      <h1>Billing demo</h1>
      <TenantSwitcher />
      <RawSubscriptionProbe />
    </main>
  );
}
