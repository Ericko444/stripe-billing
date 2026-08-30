//! Test-only helpers for producing Stripe webhook signatures.
//!
//! `#[doc(hidden)]` and kept out of the crate's real surface: this exists so
//! `webhook_signature`'s unit tests and the `tests/` integration tests can
//! share one signing implementation rather than duplicating it. It is the
//! inverse of what `webhook_signature::verify` checks, and doubles as a
//! check that the signing scheme is understood.

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Signs `payload` for `secret` at `timestamp` the way Stripe does: a hex
/// HMAC-SHA256 over `"{timestamp}.{payload}"`.
pub fn sign(payload: &str, secret: &str, timestamp: i64) -> String {
    let Ok(mut mac) = <HmacSha256 as KeyInit>::new_from_slice(secret.as_bytes()) else {
        return String::new(); // unreachable: HMAC takes a key of any length
    };
    mac.update(format!("{timestamp}.{payload}").as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Builds a full `Stripe-Signature` header value for `payload` at
/// `timestamp`, with one `v1=` entry per secret in `secrets` — so a caller
/// can emit the multi-signature header Stripe sends mid-rotation. Pass a
/// single secret for the ordinary case.
pub fn signature_header(payload: &str, timestamp: i64, secrets: &[&str]) -> String {
    let mut parts = vec![format!("t={timestamp}")];
    for secret in secrets {
        parts.push(format!("v1={}", sign(payload, secret, timestamp)));
    }
    parts.join(",")
}
