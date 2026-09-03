import { SEEDED_TENANTS, useAuth, type TenantId } from "./AuthContext";

export function TenantSwitcher() {
  const { tenantId, switchTenant } = useAuth();

  return (
    <select
      className="input"
      style={{ width: "auto", minHeight: "32px", fontSize: "13px" }}
      value={tenantId}
      onChange={(event) => switchTenant(event.target.value as TenantId)}
      aria-label="Tenant"
    >
      {Object.entries(SEEDED_TENANTS).map(([id, name]) => (
        <option key={id} value={id}>
          {name}
        </option>
      ))}
    </select>
  );
}
