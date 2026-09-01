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

use domain::{
    Currency, DomainError, Invoice, InvoiceCursor, InvoiceId, InvoicePage, Money, PaymentMethod,
    Plan, Subscription,
};
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, UtcOffset};
use uuid::Uuid;

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

/// A stored card on the wire. Display metadata only -- `brand`, `last4`,
/// `is_default`, plus id and creation time. No card number, no expiry, no
/// token: §7.4 keeps card data in Stripe and the mirror table never held
/// anything else, so this DTO's job is to not undo that by joining in
/// something richer later.
#[derive(Debug, Serialize)]
pub struct PaymentMethodDto {
    /// The local payment method id, hyphenated uuid.
    pub id: String,
    /// The card brand, e.g. `visa`.
    pub brand: String,
    /// The last four digits of the card.
    pub last4: String,
    /// Whether this is the customer's default payment method.
    pub is_default: bool,
    /// When the payment method was created, RFC 3339 UTC.
    pub created_at: String,
}

impl From<PaymentMethod> for PaymentMethodDto {
    fn from(payment_method: PaymentMethod) -> Self {
        Self {
            id: payment_method.id.as_uuid().to_string(),
            brand: payment_method.brand,
            last4: payment_method.last4,
            is_default: payment_method.is_default,
            created_at: rfc3339_utc(payment_method.created_at),
        }
    }
}

/// An invoice on the wire. `subscription_id` is the **local** id and is
/// nullable -- an invoice can exist against a customer before a subscription
/// is linked. Drops the Stripe id, the customer id and the webhook ordering
/// anchor.
#[derive(Debug, Serialize)]
pub struct InvoiceDto {
    /// The local invoice id, hyphenated uuid.
    pub id: String,
    /// The local subscription id this invoice is for, or `null`.
    pub subscription_id: Option<String>,
    /// The invoiced amount.
    pub amount: MoneyDto,
    /// Payment state: `open`, `paid` or `failed`.
    pub status: String,
    /// When the invoice was created, RFC 3339 UTC.
    pub created_at: String,
}

impl From<Invoice> for InvoiceDto {
    fn from(invoice: Invoice) -> Self {
        Self {
            id: invoice.id.as_uuid().to_string(),
            subscription_id: invoice.subscription_id.map(|id| id.as_uuid().to_string()),
            amount: invoice.amount.into(),
            status: invoice.status.as_str().to_string(),
            created_at: rfc3339_utc(invoice.created_at),
        }
    }
}

/// One page of invoices on the wire. `next` is **absent** from the JSON on
/// the last page -- not `null`, absent -- so a client's "keep paging while
/// `next` is present" loop terminates cleanly.
#[derive(Debug, Serialize)]
pub struct InvoicePageDto {
    /// The page's invoices, newest first.
    pub items: Vec<InvoiceDto>,
    /// The opaque cursor to pass as `?after=` for the next page. Omitted
    /// entirely when this is the last page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

impl From<InvoicePage> for InvoicePageDto {
    fn from(page: InvoicePage) -> Self {
        Self {
            items: page.items.into_iter().map(InvoiceDto::from).collect(),
            next: page.next.map(encode_cursor),
        }
    }
}

/// Query parameters for the paginated list routes.
///
/// Both fields are read as raw strings and interpreted by the accessors, so
/// a garbage `?limit=` degrades to the default rather than tripping Axum's
/// own (non-`problem+json`) 400 for a failed `Query` deserialization.
#[derive(Debug, Deserialize)]
pub struct PageParams {
    #[serde(default)]
    after: Option<String>,
    #[serde(default)]
    limit: Option<String>,
}

impl PageParams {
    /// The clamped page size: `1..=100`, defaulting to 25. **Never rejects**
    /// -- a 400 for `?limit=1000` is friction with no security benefit, since
    /// the clamp already bounds the work, and `?limit=0` clamps up to 1
    /// rather than paging forever over empty results. A non-numeric value is
    /// treated as absent.
    pub fn limit(&self) -> u16 {
        // Parse wide, then clamp: `?limit=100000` must clamp to 100, not
        // overflow `u16` and fall back to the default.
        let clamped = self
            .limit
            .as_deref()
            .and_then(|raw| raw.parse::<u64>().ok())
            .unwrap_or(25)
            .clamp(1, 100);
        u16::try_from(clamped).unwrap_or(100)
    }

    /// The decoded `after` cursor, if one was supplied. A malformed cursor is
    /// a [`DomainError::MalformedRequest`] -- a 400, never a panic and never
    /// a silent reset to page 1 (which would loop a paging client forever
    /// without ever telling it why).
    pub fn after(&self) -> Result<Option<InvoiceCursor>, DomainError> {
        self.after.as_deref().map(decode_cursor).transpose()
    }
}

/// Encodes an [`InvoiceCursor`] for the wire: hex of `"{micros}:{uuid}"`,
/// where `micros` is `created_at` as Unix **microseconds**.
///
/// Microseconds, not nanoseconds: `TIMESTAMPTZ` stores microsecond
/// precision, so a nanosecond cursor could not round-trip through the column
/// it indexes. Hex rather than base64 keeps the workspace's dependency set
/// unchanged (`hex` is already in it for webhook signatures); the cursor
/// only needs to be opaque, not compact.
pub fn encode_cursor(cursor: InvoiceCursor) -> String {
    let micros = cursor.created_at().unix_timestamp_nanos() / 1_000;
    hex::encode(format!("{micros}:{}", cursor.id().as_uuid()))
}

/// Decodes a wire cursor. Every failure collapses to one shape --
/// [`DomainError::MalformedRequest`] -- because to the caller they are all
/// "that cursor did not parse". The steps: hex -> UTF-8 -> split once on
/// `':'` -> parse each half.
pub fn decode_cursor(raw: &str) -> Result<InvoiceCursor, DomainError> {
    let bytes = hex::decode(raw).map_err(|_| malformed("cursor is not valid hex"))?;
    let text = String::from_utf8(bytes).map_err(|_| malformed("cursor is not valid UTF-8"))?;
    let (micros, id) = text
        .split_once(':')
        .ok_or_else(|| malformed("cursor is missing its separator"))?;
    let micros: i128 = micros
        .parse()
        .map_err(|_| malformed("cursor timestamp did not parse"))?;
    let id = Uuid::parse_str(id).map_err(|_| malformed("cursor id did not parse"))?;
    let created_at = OffsetDateTime::from_unix_timestamp_nanos(micros * 1_000)
        .map_err(|_| malformed("cursor timestamp is out of range"))?;
    Ok(InvoiceCursor::new(created_at, InvoiceId::new(id)))
}

fn malformed(detail: &str) -> DomainError {
    DomainError::MalformedRequest(detail.to_string())
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

    fn sample_cursor() -> InvoiceCursor {
        // A microsecond-aligned instant, so it survives the codec's
        // micros round-trip exactly.
        let at = OffsetDateTime::from_unix_timestamp_nanos(1_712_000_000_000_000)
            .unwrap_or(OffsetDateTime::UNIX_EPOCH);
        InvoiceCursor::new(at, InvoiceId::new(Uuid::from_u128(0x1234_5678_9abc_def0)))
    }

    #[test]
    fn cursor_round_trips() {
        let cursor = sample_cursor();
        assert_eq!(
            decode_cursor(&encode_cursor(cursor)).ok(),
            Some(cursor),
            "decode(encode(c)) == c",
        );
    }

    #[test]
    fn a_malformed_cursor_is_a_domain_malformed_request() {
        for bad in [
            "not-hex-zz",
            "",
            &hex::encode("no-colon-here"),
            &hex::encode("123:not-a-uuid"),
        ] {
            assert!(
                matches!(decode_cursor(bad), Err(DomainError::MalformedRequest(_))),
                "{bad:?} should decode to MalformedRequest",
            );
        }
    }

    #[test]
    fn limit_clamps_and_never_rejects() {
        let params = |raw: Option<&str>| PageParams {
            after: None,
            limit: raw.map(str::to_string),
        };
        assert_eq!(params(None).limit(), 25);
        assert_eq!(params(Some("50")).limit(), 50);
        assert_eq!(params(Some("0")).limit(), 1);
        assert_eq!(params(Some("100000")).limit(), 100);
        assert_eq!(params(Some("garbage")).limit(), 25);
    }

    #[test]
    fn next_is_absent_from_json_on_the_last_page() {
        let page = InvoicePageDto {
            items: Vec::new(),
            next: None,
        };
        let rendered = serde_json::to_string(&page).unwrap_or_default();
        assert_eq!(rendered, r#"{"items":[]}"#);
    }
}
