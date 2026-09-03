import { SEEDED_TENANTS, useAuth, type TenantId } from "./AuthContext";

export function TenantSwitcher() {
  const { tenantId, switchTenant } = useAuth();

  return (
    <label className="flex items-center gap-2.5 rounded-xl border border-slate-200 bg-slate-50 py-1.5 pl-3 pr-1.5">
      <span className="text-[11px] font-semibold uppercase tracking-[0.08em] text-slate-500">
        Tenant
      </span>
      <select
        value={tenantId}
        onChange={(event) => switchTenant(event.target.value as TenantId)}
        className="cursor-pointer rounded-lg border border-slate-300 bg-white px-2.5 py-1.5 text-sm font-medium text-slate-800 shadow-sm transition-colors hover:bg-slate-50 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-indigo-600"
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
