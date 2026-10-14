//! The seam satisfies billing's contract, checked through the paths a
//! consumer of this crate writes.
//!
//! The assertion is the compile: if `IdentityTenant` stopped satisfying
//! `TenantExtractor`, this file would not build. What the seam does at
//! request time -- every refusal becoming billing's one `401`, a session
//! reaching a billing handler as its tenant -- is exercised by `demo`'s
//! integration tests against a real Postgres, on this same type.

use tenantry::IdentityTenant;
use tenantry::billing::api::TenantExtractor;

fn assert_tenant_extractor<T: TenantExtractor>() {}

#[test]
fn identity_tenant_satisfies_billings_tenant_extractor() {
    assert_tenant_extractor::<IdentityTenant>();
}
