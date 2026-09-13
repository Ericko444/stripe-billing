import { ApiProblem } from "../../host/api/apiClient";
import type { MembershipDto } from "../api/types";
import { Alert, Badge, Button } from "../../ui/primitives";

/**
 * The step between logging in and using a tenant, for someone who belongs to
 * several. Presentational: the host owns what picking does (rotating the
 * session and dropping other tenants' cached data), so this component only
 * reports the choice.
 *
 * While a pick is in flight every choice is disabled -- one rotation at a
 * time -- and a failed pick shows its `detail` with the choices open again.
 */
export function TenantPicker({
  memberships,
  onPick,
  pending,
  problem,
  onLogout,
}: {
  memberships: MembershipDto[];
  onPick: (tenantId: string) => void;
  pending: boolean;
  problem: Error | null;
  onLogout: () => void;
}) {
  return (
    <div className="card elev-md stack" style={{ padding: "24px", width: "100%", maxWidth: "420px" }}>
      <div>
        <h1 className="panel-title is-prominent">Choose a tenant</h1>
        <p className="panel-sub">You belong to more than one. You can switch later.</p>
      </div>

      <ul className="stack" style={{ listStyle: "none", margin: 0, padding: 0 }} aria-busy={pending}>
        {memberships.map((membership) => (
          <li key={membership.tenant_id}>
            <Button
              block
              disabled={pending}
              onClick={() => onPick(membership.tenant_id)}
              style={{ justifyContent: "space-between", display: "flex" }}
            >
              <span>{membership.tenant_name}</span>
              <Badge tone="neutral">{membership.role}</Badge>
            </Button>
          </li>
        ))}
      </ul>

      {problem && (
        <Alert>
          {problem instanceof ApiProblem && problem.detail
            ? problem.detail
            : "Could not open that tenant. Try again."}
        </Alert>
      )}

      <Button variant="ghost" size="sm" onClick={onLogout} disabled={pending}>
        Log out
      </Button>
    </div>
  );
}
