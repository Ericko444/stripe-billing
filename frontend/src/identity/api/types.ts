/**
 * Hand-written mirrors of `crates/identity-api/src/dto.rs`, the same
 * discipline as `billing/api/types.ts`: one file, no codegen, reviewed next
 * to the Rust it mirrors.
 *
 * There is no token anywhere in these types. The session travels in an
 * `HttpOnly` cookie the browser attaches itself; script never sees it.
 */

export type Role = "owner" | "admin" | "member";

export interface MembershipDto {
  membership_id: string;
  tenant_id: string;
  tenant_name: string;
  role: Role;
}

export interface CurrentTenantDto {
  tenant_id: string;
  tenant_name: string;
  role: Role;
}

/** `POST /auth/login` and `POST /auth/tenant`. */
export interface SessionDto {
  user_id: string;
  /** `null` until a tenant is picked. */
  tenant: CurrentTenantDto | null;
  memberships: MembershipDto[];
  expires_at: string;
}

/** `GET` and `PATCH /auth/me`. */
export interface MeDto {
  user_id: string;
  email: string;
  display_name: string;
  tenant: CurrentTenantDto | null;
  memberships: MembershipDto[];
  expires_at: string;
}

export interface LoginRequest {
  email: string;
  password: string;
}
