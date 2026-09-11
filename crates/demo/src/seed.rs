//! `cargo run -p demo -- seed`: gives the tenant switcher two tenants to
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
use persistence::{
    PgCustomerRepository, PgOutboundRequestRepository, PgPlanRepository, PgSubscriptionRepository,
    run_migrations,
};
use secrecy::SecretString;
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

/// Runs the seed: connects to Postgres and Stripe, then seeds each of
/// [`SEED_TENANTS`] in turn, printing each tenant's uuid as it completes.
pub async fn run_seed() -> Result<(), Box<dyn Error>> {
    let database_url = required_env("DATABASE_URL")?;
    let stripe_secret_key = required_env("STRIPE_SECRET_KEY")?;
    let pricing = SeedPricing::from_env()?;

    let pool = PgPoolOptions::new().connect(&database_url).await?;
    run_migrations(&pool).await?;

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
