import { describe, expect, test, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { TenantPicker } from "./TenantPicker";
import type { MembershipDto } from "../api/types";

const MEMBERSHIPS: MembershipDto[] = [
  { membership_id: "m-a", tenant_id: "t-a", tenant_name: "Tenant A", role: "owner" },
  { membership_id: "m-b", tenant_id: "t-b", tenant_name: "Tenant B", role: "member" },
];

describe("TenantPicker", () => {
  test("lists every membership and reports the one picked", () => {
    const onPick = vi.fn();
    render(
      <TenantPicker
        memberships={MEMBERSHIPS}
        onPick={onPick}
        pending={false}
        problem={null}
        onLogout={vi.fn()}
      />,
    );

    expect(screen.getByRole("button", { name: /tenant a/i })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /tenant b/i }));

    expect(onPick).toHaveBeenCalledWith("t-b");
  });

  test("disables every choice while a pick is in flight", () => {
    render(
      <TenantPicker
        memberships={MEMBERSHIPS}
        onPick={vi.fn()}
        pending
        problem={null}
        onLogout={vi.fn()}
      />,
    );

    expect(screen.getByRole("button", { name: /tenant a/i })).toBeDisabled();
    expect(screen.getByRole("button", { name: /tenant b/i })).toBeDisabled();
  });
});
