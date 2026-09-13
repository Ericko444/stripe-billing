import { useAuth, useSessionState } from "./AuthContext";

/**
 * Switches the session between the caller's own memberships -- the list
 * comes from the session, not from a hard-coded seed. Picking one asks the
 * identity module to rotate the session onto that tenant; the options are
 * disabled while that is in flight, so a second pick cannot race the first.
 * With a single membership there is nothing to switch, and nothing is shown.
 */
export function TenantSwitcher() {
  const { tenantId, memberships, switchTenant } = useAuth();
  const { isSwitching } = useSessionState();

  if (memberships.length < 2) {
    return null;
  }

  return (
    <select
      className="input"
      style={{ width: "auto", minHeight: "32px", fontSize: "13px" }}
      value={tenantId}
      disabled={isSwitching}
      aria-busy={isSwitching}
      onChange={(event) => switchTenant(event.target.value)}
      aria-label="Tenant"
    >
      {memberships.map((membership) => (
        <option key={membership.tenant_id} value={membership.tenant_id}>
          {membership.tenant_name}
        </option>
      ))}
    </select>
  );
}
