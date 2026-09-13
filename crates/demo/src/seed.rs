//! `cargo run -p demo -- seed`: gives the demo two users and two tenants to
//! switch between.
//!
//! Per tenant: a Stripe customer plus its mirror row, two local `plans`
//! rows against Stripe prices the operator names (`SEED_PLAN_*`, below),
//! and one subscription. Tenant ids are **fixed**, not random -- a second
//! run must find the same two tenants rather than minting new ones, which
//! is what makes re-running idempotent.
//!
//! No `billing.payment_methods` row is written. `BillingProvider` has no
//! `attach_payment_method` -- attaching has only ever happened through
//! Stripe's hosted UI -- so a subscription created here comes back
//! `incomplete`, not `active`, until a reviewer runs checkout or a
//! SetupIntent themselves. That is the honest starting
//! state, not a bug.

use std::error::Error;

use domain::{
    CreateCustomerParams, Currency, CustomerRepository, Money, Plan, PlanRepository,
    SubscriptionRepository, TenantId,
};
use identity_domain::{NewPassword, Password, PasswordHasher};
use identity_service::Argon2Hasher;
use persistence::{
    PgCustomerRepository, PgOutboundRequestRepository, PgPlanRepository, PgSubscriptionRepository,
    run_migrations,
};
use secrecy::SecretString;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use stripe_adapter::{StripeBillingProvider, StripeConfig};
use uuid::Uuid;

/// The two demo tenants. Fixed rather than random so the seed is
/// idempotent: a second run recognizes these ids and finds the rows the
/// first run created instead of minting new tenants.
const SEED_TENANTS: [Uuid; 2] = [
    Uuid::from_u128(0x0000_0000_0000_0000_0000_0000_0000_0001),
    Uuid::from_u128(0x0000_0000_0000_0000_0000_0000_0000_0002),
];

/// Reads a required variable, returning an error naming it rather than
/// panicking -- the workspace denies `unwrap`/`expect`/`panic`.
fn required_env(name: &str) -> Result<String, Box<dyn Error>> {
    match std::env::var(name) {
        Ok(value) if !value.is_empty() => Ok(value),
        _ => Err(format!("required environment variable {name} is not set").into()),
    }
}

/// Seeds the identity module: the two tenants (the same ids billing uses),
/// Alice (Owner of A, Member of B) and Bob (Owner of B), both with the
/// password in `SEED_USER_PASSWORD`, hashed with the live Argon2id
/// parameters.
///
/// Written with SQL rather than through a use case, deliberately: seeding
/// is setup, not an action anyone took, so it writes no audit rows. Re-running
/// leaves tenants and memberships as they are but **resets both users'
/// passwords** to the current `SEED_USER_PASSWORD` -- convenient in a demo,
/// and the reason this command must never point at a real database.
async fn seed_identity(pool: &PgPool, password: Password) -> Result<(), Box<dyn Error>> {
    let password = NewPassword::check(password)
        .map_err(|err| format!("SEED_USER_PASSWORD is not an acceptable password: {err}"))?;
    let hash = Argon2Hasher::new()?.hash(&password).await?;

    let mut tx = pool.begin().await?;
    for (id, name) in [(SEED_TENANTS[0], "Tenant A"), (SEED_TENANTS[1], "Tenant B")] {
        sqlx::query(
            "INSERT INTO identity.tenants (id, name) VALUES ($1, $2) ON CONFLICT (id) DO NOTHING",
        )
        .bind(id)
        .bind(name)
        .execute(&mut *tx)
        .await?;
    }
    for (id, email, name) in [
        (SEED_ALICE, "alice@example.test", "Alice"),
        (SEED_BOB, "bob@example.test", "Bob"),
    ] {
        sqlx::query(
            "INSERT INTO identity.users (id, email_normalized, display_name, password_hash) \
             VALUES ($1, $2, $3, $4) \
             ON CONFLICT (id) DO UPDATE SET password_hash = EXCLUDED.password_hash",
        )
        .bind(id)
        .bind(email)
        .bind(name)
        .bind(hash.as_str())
        .execute(&mut *tx)
        .await?;
    }
    for (user, tenant, role) in [
        (SEED_ALICE, SEED_TENANTS[0], "owner"),
        (SEED_ALICE, SEED_TENANTS[1], "member"),
        (SEED_BOB, SEED_TENANTS[1], "owner"),
    ] {
        sqlx::query(
            "INSERT INTO identity.memberships (id, user_id, tenant_id, role, status) \
             VALUES ($1, $2, $3, $4, 'active') \
             ON CONFLICT (user_id, tenant_id) DO NOTHING",
        )
        .bind(Uuid::new_v4())
        .bind(user)
        .bind(tenant)
        .bind(role)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    println!(
        "seeded users alice@example.test (Tenant A owner, Tenant B member) and bob@example.test (Tenant B owner)"
    );
    Ok(())
}

/// The two shared test-mode Stripe prices the seed's plans point at. Named
/// by environment variable rather than hardcoded: unlike a Stripe secret
/// key, a price/product id is not something this crate can verify, and a
/// wrong hardcoded id would leave a mirror row pointing at nothing. Point
/// these at any two prices in
/// your own Stripe test-mode account.
struct SeedPricing {
    plan_a_price_id: String,
    plan_a_product_id: String,
    plan_b_price_id: String,
    plan_b_product_id: String,
}

impl SeedPricing {
    fn from_env() -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            plan_a_price_id: required_env("SEED_PLAN_A_STRIPE_PRICE_ID")?,
            plan_a_product_id: required_env("SEED_PLAN_A_STRIPE_PRODUCT_ID")?,
            plan_b_price_id: required_env("SEED_PLAN_B_STRIPE_PRICE_ID")?,
            plan_b_product_id: required_env("SEED_PLAN_B_STRIPE_PRODUCT_ID")?,
        })
    }
}

/// The two demo users. Fixed ids, for the same idempotency reason as the
/// tenants. Alice belongs to both tenants -- she exercises the tenant picker
/// -- and Bob to one, so he lands straight in it.
const SEED_ALICE: Uuid = Uuid::from_u128(0x0000_0000_0000_0000_0000_0000_000a_11ce);
const SEED_BOB: Uuid = Uuid::from_u128(0x0000_0000_0000_0000_0000_0000_0000_0b0b);

/// Runs the seed: the identity module's tenants, users and memberships
/// first (Postgres only), then each of [`SEED_TENANTS`]'s billing state
/// (Postgres and Stripe), printing each tenant's uuid as it completes.
pub async fn run_seed() -> Result<(), Box<dyn Error>> {
    let database_url = required_env("DATABASE_URL")?;
    let seed_password = Password::new(required_env("SEED_USER_PASSWORD")?);

    let pool = PgPoolOptions::new().connect(&database_url).await?;
    run_migrations(&pool).await?;
    audit_pg::run_migrations(&pool).await?;
    identity_pg::run_migrations(&pool).await?;
    seed_identity(&pool, seed_password).await?;

    let stripe_secret_key = required_env("STRIPE_SECRET_KEY")?;
    let pricing = SeedPricing::from_env()?;

    let customers = PgCustomerRepository::new(pool.clone());
    let plans = PgPlanRepository::new(pool.clone());
    let subscriptions = PgSubscriptionRepository::new(pool.clone());
    let provider = StripeBillingProvider::new(
        &StripeConfig {
            secret: SecretString::from(stripe_secret_key),
            base_url: None,
        },
        PgOutboundRequestRepository::new(pool.clone()),
    )?;

    for tenant_uuid in SEED_TENANTS {
        let tenant = TenantId::new(tenant_uuid);
        seed_tenant(
            tenant,
            &provider,
            &customers,
            &plans,
            &subscriptions,
            &pricing,
        )
        .await?;
        println!("seeded tenant {tenant_uuid}");
    }

    Ok(())
}

/// Seeds one tenant: a customer, two plans, one subscription -- each step
/// skipped if a prior run already created it, so the whole function is
/// idempotent.
async fn seed_tenant(
    tenant: TenantId,
    provider: &StripeBillingProvider<PgOutboundRequestRepository>,
    customers: &PgCustomerRepository,
    plans: &PgPlanRepository,
    subscriptions: &PgSubscriptionRepository,
    pricing: &SeedPricing,
) -> Result<(), Box<dyn Error>> {
    let (customer_id, stripe_customer_id) = match customers.list(tenant).await?.into_iter().next() {
        Some(existing) => {
            let stripe_id = existing
                .stripe_customer_id
                .ok_or("a prior seed run's customer row has no stripe_customer_id")?;
            (existing.id, stripe_id)
        }
        None => {
            let snapshot = provider
                .create_customer(
                    tenant,
                    CreateCustomerParams {
                        email: None,
                        name: None,
                    },
                )
                .await?;
            let customer = customers
                .create(tenant, Some(snapshot.stripe_customer_id.clone()))
                .await?;
            (customer.id, snapshot.stripe_customer_id)
        }
    };

    let existing_plans = plans.list(tenant).await?;
    let plan_a = seed_plan(
        plans,
        tenant,
        &existing_plans,
        &pricing.plan_a_price_id,
        &pricing.plan_a_product_id,
        "Starter",
        1500,
    )
    .await?;
    seed_plan(
        plans,
        tenant,
        &existing_plans,
        &pricing.plan_b_price_id,
        &pricing.plan_b_product_id,
        "Pro",
        4900,
    )
    .await?;

    if subscriptions.list(tenant).await?.is_empty() {
        let snapshot = provider
            .create_subscription(tenant, &stripe_customer_id, &pricing.plan_a_price_id)
            .await?;
        // Stored as Stripe returned it, never coerced to `Active` -- a
        // subscription with no attached payment method comes back
        // `incomplete`, and that is the honest state to mirror.
        subscriptions
            .create(
                tenant,
                customer_id,
                plan_a.id,
                snapshot.stripe_subscription_id,
                snapshot.stripe_subscription_item_id,
                snapshot.status,
                snapshot.current_period_start,
                snapshot.current_period_end,
            )
            .await?;
    }

    Ok(())
}

/// Finds `price_id` among `existing`, or creates a new plan row for it.
#[allow(clippy::too_many_arguments)]
async fn seed_plan(
    plans: &PgPlanRepository,
    tenant: TenantId,
    existing: &[Plan],
    price_id: &str,
    product_id: &str,
    name: &str,
    amount_cents: i64,
) -> Result<Plan, Box<dyn Error>> {
    if let Some(plan) = existing.iter().find(|p| p.stripe_price_id == price_id) {
        return Ok(plan.clone());
    }
    Ok(plans
        .create(
            tenant,
            price_id.to_string(),
            product_id.to_string(),
            name.to_string(),
            Money::new(amount_cents, Currency::Usd),
        )
        .await?)
}
