import { SEEDED_TENANTS, useAuth, type TenantId } from "./AuthContext";

export function TenantSwitcher() {
  const { tenantId, switchTenant } = useAuth();

  return (
    <label>
      Tenant:{" "}
      <select
        value={tenantId}
        onChange={(event) => switchTenant(event.target.value as TenantId)}
      >
        {Object.entries(SEEDED_TENANTS).map(([id, name]) => (
          <option key={id} value={id}>
            {name}
          </option>
        ))}
      </select>
    </label>
  );
}
