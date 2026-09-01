//! Wire types, distinct from the domain model.
//!
//! §9: DTOs are defined here and are **not** the domain types, so the wire
//! format can evolve without a domain change forcing it, and a field added
//! to a domain struct never silently becomes a wire-breaking change. The
//! conversion is an explicit `From` impl per type and it is allowed to be
//! tedious.
//!
//! No domain type derives or implements `Serialize`. Money is
//! `{amount_minor, currency}` -- never a float (which cannot hold 19.99
//! exactly), never a pre-formatted `"€19.99"` (which forces every consumer
//! to parse a locale back out). Timestamps are RFC 3339 UTC strings.

use domain::{Currency, Money, Plan, Subscription};
use serde::Serialize;
use time::{OffsetDateTime, UtcOffset};

/// A monetary amount on the wire: an integer count of the currency's minor
/// unit, plus the ISO 4217 code as a separate field (S5).
#[derive(Debug, Serialize)]
pub struct MoneyDto {
    /// The amount in the currency's smallest unit, e.g. `1999` for €19.99.
    pub amount_minor: i64,
    /// ISO 4217 code, uppercase.
    pub currency: String,
}

impl From<Money> for MoneyDto {
    fn from(money: Money) -> Self {
        Self {
            amount_minor: money.amount_minor(),
            currency: currency_code(money.currency()).to_string(),
        }
    }
}

/// A plan on the wire. Drops the Stripe price/product ids and the
/// soft-delete marker -- a consumer of the catalogue needs neither.
#[derive(Debug, Serialize)]
pub struct PlanDto {
    /// The local plan id, hyphenated uuid.
    pub id: String,
    /// Human-readable plan name.
    pub name: String,
    /// The plan's price.
    pub amount: MoneyDto,
    /// When the plan was created, RFC 3339 UTC.
    pub created_at: String,
}

impl From<Plan> for PlanDto {
    fn from(plan: Plan) -> Self {
        Self {
            id: plan.id.as_uuid().to_string(),
            name: plan.name,
            amount: plan.amount.into(),
            created_at: rfc3339_utc(plan.created_at),
        }
    }
}

/// A subscription on the wire. `plan_id` is the **local** plan id, never the
/// Stripe price id -- a consumer joins it against `GET /plans`, not against
/// anything in Stripe. Drops the Stripe ids, the customer id and the webhook
/// ordering anchor.
#[derive(Debug, Serialize)]
pub struct SubscriptionDto {
    /// The local subscription id, hyphenated uuid.
    pub id: String,
    /// The local plan id this subscription is for.
    pub plan_id: String,
    /// Lifecycle state: `active`, `past_due`, `canceled` or `incomplete`.
    pub status: String,
    /// Start of the current billing period, RFC 3339 UTC.
    pub current_period_start: String,
    /// End of the current billing period, RFC 3339 UTC.
    pub current_period_end: String,
    /// Whether the subscription is set to end at the period boundary.
    pub cancel_at_period_end: bool,
    /// When the subscription was created, RFC 3339 UTC.
    pub created_at: String,
}

impl From<Subscription> for SubscriptionDto {
    fn from(subscription: Subscription) -> Self {
        Self {
            id: subscription.id.as_uuid().to_string(),
            plan_id: subscription.plan_id.as_uuid().to_string(),
            status: subscription.status.as_str().to_string(),
            current_period_start: rfc3339_utc(subscription.current_period_start),
            current_period_end: rfc3339_utc(subscription.current_period_end),
            cancel_at_period_end: subscription.cancel_at_period_end,
            created_at: rfc3339_utc(subscription.created_at),
        }
    }
}

/// ISO 4217 code for a [`Currency`]. A `match`, not a `Display` impl on the
/// domain enum: the wire spelling is `api`'s concern, not `domain`'s.
fn currency_code(currency: Currency) -> &'static str {
    match currency {
        Currency::Usd => "USD",
        Currency::Eur => "EUR",
        Currency::Gbp => "GBP",
    }
}

/// Formats an [`OffsetDateTime`] as an RFC 3339 UTC string with second
/// precision (`2026-09-01T12:34:56Z`).
///
/// Built from the datetime's own fields rather than `time`'s `Rfc3339`
/// formatter so the conversion stays infallible and needs no extra `time`
/// feature. Sub-second precision is dropped, which RFC 3339 permits.
fn rfc3339_utc(datetime: OffsetDateTime) -> String {
    let datetime = datetime.to_offset(UtcOffset::UTC);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        datetime.year(),
        u8::from(datetime.month()),
        datetime.day(),
        datetime.hour(),
        datetime.minute(),
        datetime.second(),
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn money_serializes_as_minor_units_and_iso_code() {
        let dto = MoneyDto::from(Money::new(1999, Currency::Eur));
        assert_eq!(
            serde_json::to_value(&dto).unwrap_or(serde_json::Value::Null),
            json!({ "amount_minor": 1999, "currency": "EUR" }),
        );
    }

    #[test]
    fn money_dto_has_no_float() {
        let rendered = serde_json::to_string(&MoneyDto::from(Money::new(1999, Currency::Eur)))
            .unwrap_or_default();
        assert!(rendered.contains("\"amount_minor\":1999"));
        assert!(!rendered.contains("19.99"));
        assert!(!rendered.contains('.'));
    }

    #[test]
    fn timestamp_is_rfc3339_utc_and_drops_sub_second() {
        // 1.75s past the epoch -- the fractional part must not survive.
        let dt = OffsetDateTime::from_unix_timestamp_nanos(1_750_000_000)
            .unwrap_or(OffsetDateTime::UNIX_EPOCH);
        assert_eq!(rfc3339_utc(dt), "1970-01-01T00:00:01Z");
    }
}
