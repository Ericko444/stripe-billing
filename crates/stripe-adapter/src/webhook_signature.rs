//! Stripe webhook signature verification, hand-rolled over RustCrypto
//! primitives.
//!
//! Pure: no I/O, no repository, no clock. [`verify`] takes `now` as a
//! parameter, so the replay-window tests need no sleeping and no clock trait
//! leaks into production code. The public path passes
//! `OffsetDateTime::now_utc()`.
//!
//! **This module implements no cryptography.** The MAC is `Hmac<Sha256>` and
//! the comparison is `hmac::Mac::verify_slice`, which is constant-time. What
//! it owns is header parsing and one integer comparison.
//!
//! **A delivery is accepted if *any* `v1` signature in the header matches.**
//! Stripe sends multiple `v1` signatures while an endpoint secret is being
//! rotated — the old secret stays valid for about a day, and the header
//! carries one signature per active secret. Checking only the last one — the
//! defect that disqualified `async-stripe-webhook` — rejects valid
//! deliveries for the duration of every rotation. The named regression test
//! `accepts_when_only_one_of_two_v1_matches` pins this and should fail if
//! someone "simplifies" `v1` from a `Vec` to a single value.

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use time::{Duration, OffsetDateTime};

use crate::webhook_error::WebhookError;

type HmacSha256 = Hmac<Sha256>;

/// Stripe's recommended replay window (5 minutes). A parameter to [`verify`]
/// and a field on `WebhookConfig`, not a value hardcoded in the verifier the
/// way the rejected SDK's was — hand-rolling made the tolerance
/// configurable.
pub const DEFAULT_TOLERANCE: Duration = Duration::minutes(5);

/// The parsed `Stripe-Signature` header: the `t` timestamp and *every* `v1`
/// signature, not merely the last. Unknown schemes (`v0=`, a future `v2=`)
/// are dropped during parsing, per Stripe's documented
/// forward-compatibility rule.
struct SignatureHeader {
    timestamp: i64,
    v1: Vec<String>,
}

impl SignatureHeader {
    /// Parses `t=<unix seconds>,v1=<hex>,v1=<hex>,…`. A missing or
    /// unparseable `t`, or zero `v1` values, is a
    /// [`WebhookError::MalformedHeader`].
    fn parse(header: &str) -> Result<Self, WebhookError> {
        let mut timestamp: Option<i64> = None;
        let mut v1 = Vec::new();

        for element in header.split(',') {
            let Some((key, value)) = element.split_once('=') else {
                continue;
            };
            match key.trim() {
                "t" => {
                    let parsed = value.trim().parse().map_err(|_| {
                        WebhookError::MalformedHeader(format!("unparseable t: {value:?}"))
                    })?;
                    timestamp = Some(parsed);
                }
                "v1" => v1.push(value.trim().to_string()),
                _ => {} // unknown scheme — ignored, not fatal
            }
        }

        let Some(timestamp) = timestamp else {
            return Err(WebhookError::MalformedHeader("missing t".to_string()));
        };
        if v1.is_empty() {
            return Err(WebhookError::MalformedHeader("no v1 signature".to_string()));
        }

        Ok(Self { timestamp, v1 })
    }
}

/// Verifies `payload` against its `Stripe-Signature` `header`.
///
/// Order of operations, itself a decision:
/// 1. Parse the header — collect `t` and every `v1`, ignore unknown schemes.
/// 2. Verify the signature: HMAC-SHA256 of `"{t}.{payload}"` keyed with
///    `secret`, accepted if **any** `v1` matches. The comparison is
///    delegated to `Mac::verify_slice` (constant-time); this module never
///    compares signature bytes itself.
/// 3. Check the replay window: reject if `|now - t| > tolerance`.
///
/// Signature before timestamp, matching Stripe's own libraries:
/// authenticate first, judge freshness second.
pub(crate) fn verify(
    payload: &str,
    header: &str,
    secret: &str,
    tolerance: Duration,
    now: OffsetDateTime,
) -> Result<(), WebhookError> {
    let parsed = SignatureHeader::parse(header)?;

    verify_signature(payload, parsed.timestamp, &parsed.v1, secret)?;
    check_replay_window(parsed.timestamp, tolerance, now)?;

    Ok(())
}

/// Accepts if *any* candidate is a valid hex HMAC-SHA256 of
/// `"{timestamp}.{payload}"` under `secret`. A candidate that is not valid
/// hex simply does not match — it is not a distinct error.
fn verify_signature(
    payload: &str,
    timestamp: i64,
    candidates: &[String],
    secret: &str,
) -> Result<(), WebhookError> {
    let signed_payload = format!("{timestamp}.{payload}");

    // HMAC accepts a key of any length, so `new_from_slice` cannot actually
    // fail here — but this crate denies `unwrap`, and to a caller a failed
    // MAC construction is indistinguishable from a bad signature anyway.
    let mut mac = <HmacSha256 as KeyInit>::new_from_slice(secret.as_bytes())
        .map_err(|_| WebhookError::Signature)?;
    mac.update(signed_payload.as_bytes());

    let any_match = candidates.iter().any(|candidate| {
        hex::decode(candidate)
            .map(|decoded| mac.clone().verify_slice(&decoded).is_ok())
            .unwrap_or(false)
    });

    if any_match {
        Ok(())
    } else {
        Err(WebhookError::Signature)
    }
}

/// Rejects with [`WebhookError::Timestamp`] if the header time is more than
/// `tolerance` away from `now` in either direction — a stale replay or a
/// timestamp implausibly far in the future.
fn check_replay_window(
    timestamp: i64,
    tolerance: Duration,
    now: OffsetDateTime,
) -> Result<(), WebhookError> {
    let event_time = OffsetDateTime::from_unix_timestamp(timestamp)
        .map_err(|_| WebhookError::MalformedHeader(format!("t out of range: {timestamp}")))?;

    let skew = (now - event_time).abs();
    if skew > tolerance {
        Err(WebhookError::Timestamp(format!(
            "event {skew} from now, tolerance {tolerance}"
        )))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Signs `payload` for `secret` at `timestamp` exactly as Stripe does:
    /// hex HMAC-SHA256 over `"{timestamp}.{payload}"`. Multi-`v1` headers are
    /// built by calling this with different secrets and joining the results
    /// with `,v1=`.
    fn sign(payload: &str, secret: &str, timestamp: i64) -> String {
        let Ok(mut mac) = <HmacSha256 as KeyInit>::new_from_slice(secret.as_bytes()) else {
            return String::new(); // unreachable: HMAC takes a key of any length
        };
        mac.update(format!("{timestamp}.{payload}").as_bytes());
        hex::encode(mac.finalize().into_bytes())
    }

    /// A concrete `OffsetDateTime` `unix` seconds after the epoch, without a
    /// fallible `from_unix_timestamp` call in every test body.
    fn at(unix: i64) -> OffsetDateTime {
        OffsetDateTime::UNIX_EPOCH + Duration::seconds(unix)
    }

    const SECRET: &str = "whsec_test_secret";
    const PAYLOAD: &str = r#"{"id":"evt_123","type":"customer.created"}"#;
    const TS: i64 = 1_700_000_000;

    #[test]
    fn valid_signature_is_accepted() {
        let sig = sign(PAYLOAD, SECRET, TS);
        let header = format!("t={TS},v1={sig}");
        assert!(verify(PAYLOAD, &header, SECRET, DEFAULT_TOLERANCE, at(TS)).is_ok());
    }

    #[test]
    fn tampered_payload_is_rejected() {
        let sig = sign(PAYLOAD, SECRET, TS);
        let header = format!("t={TS},v1={sig}");
        let tampered = r#"{"id":"evt_123","type":"customer.deleted"}"#;
        assert!(matches!(
            verify(tampered, &header, SECRET, DEFAULT_TOLERANCE, at(TS)),
            Err(WebhookError::Signature)
        ));
    }

    #[test]
    fn accepts_when_only_one_of_two_v1_matches() {
        // The named regression guard for the defect that disqualified
        // async-stripe-webhook: it kept only the last v1, so during a secret
        // rotation — when Stripe sends one signature per active secret — a
        // valid delivery would be rejected. We accept if ANY v1 matches, and
        // the position of the matching one must not matter.
        let good = sign(PAYLOAD, SECRET, TS);
        let bad = sign(PAYLOAD, "whsec_the_other_secret", TS);

        for header in [
            format!("t={TS},v1={bad},v1={good}"),
            format!("t={TS},v1={good},v1={bad}"),
        ] {
            assert!(
                verify(PAYLOAD, &header, SECRET, DEFAULT_TOLERANCE, at(TS)).is_ok(),
                "a header with one matching v1 must be accepted: {header}"
            );
        }
    }

    #[test]
    fn rejects_when_neither_of_two_v1_matches() {
        let bad1 = sign(PAYLOAD, "whsec_wrong_1", TS);
        let bad2 = sign(PAYLOAD, "whsec_wrong_2", TS);
        let header = format!("t={TS},v1={bad1},v1={bad2}");
        assert!(matches!(
            verify(PAYLOAD, &header, SECRET, DEFAULT_TOLERANCE, at(TS)),
            Err(WebhookError::Signature)
        ));
    }

    #[test]
    fn expired_timestamp_is_rejected() {
        let sig = sign(PAYLOAD, SECRET, TS);
        let header = format!("t={TS},v1={sig}");
        // now is 10 minutes after the event; tolerance is 5.
        let now = at(TS) + Duration::minutes(10);
        assert!(matches!(
            verify(PAYLOAD, &header, SECRET, DEFAULT_TOLERANCE, now),
            Err(WebhookError::Timestamp(_))
        ));
    }

    #[test]
    fn future_timestamp_beyond_tolerance_is_rejected() {
        let sig = sign(PAYLOAD, SECRET, TS);
        let header = format!("t={TS},v1={sig}");
        // now is 10 minutes *before* the event — clock skew the other way.
        let now = at(TS) - Duration::minutes(10);
        assert!(matches!(
            verify(PAYLOAD, &header, SECRET, DEFAULT_TOLERANCE, now),
            Err(WebhookError::Timestamp(_))
        ));
    }

    #[test]
    fn malformed_headers_are_rejected() {
        let sig = sign(PAYLOAD, SECRET, TS);
        let cases = [
            ("missing t", format!("v1={sig}")),
            ("missing v1", format!("t={TS}")),
            ("unparseable t", format!("t=not-a-number,v1={sig}")),
            ("garbage", "complete garbage with no equals".to_string()),
        ];
        for (name, header) in cases {
            assert!(
                matches!(
                    verify(PAYLOAD, &header, SECRET, DEFAULT_TOLERANCE, at(TS)),
                    Err(WebhookError::MalformedHeader(_))
                ),
                "{name} header must be rejected as malformed: {header:?}"
            );
        }
    }

    #[test]
    fn unknown_scheme_alongside_valid_v1_is_ignored() {
        let sig = sign(PAYLOAD, SECRET, TS);
        // The legacy v0=, plus a hypothetical future v2=, alongside a good v1.
        let header = format!("t={TS},v0=deadbeef,v2=whatever,v1={sig}");
        assert!(
            verify(PAYLOAD, &header, SECRET, DEFAULT_TOLERANCE, at(TS)).is_ok(),
            "unknown schemes must be ignored, not fatal"
        );
    }
}
