use core::fmt::Write as _;

use sha2::{Digest, Sha256};

/// Hashes an operation name and its input fields into a stable, 64-character
/// lowercase hex fingerprint, used to detect a retried (or, if the inputs
/// changed, mismatched) idempotency ledger reservation.
///
/// Each field is encoded as `{byte length}:{value}` before hashing --
/// length-prefixed, not delimiter-joined. A `|`-joined encoding is
/// injectable: a customer name containing `|` could make two different
/// input sets produce the same string and therefore the same fingerprint,
/// which Stripe would then reject as a mismatched reuse -- and it would be
/// this module's bug, not Stripe's. Length prefixing makes the encoding
/// unambiguous by construction: no value, however it's punctuated, can make
/// two different field lists collide.
///
/// **Every input that varies the Stripe request body must be passed in
/// `fields`.** An input that reaches Stripe but not the fingerprint means
/// two logically different requests can share a ledger row and, with it, an
/// idempotency key -- this is a per-call-site review item, not something
/// this function can check for its caller.
pub fn fingerprint(operation: &str, fields: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for field in core::iter::once(operation).chain(fields.iter().copied()) {
        hasher.update(field.len().to_string().as_bytes());
        hasher.update(b":");
        hasher.update(field.as_bytes());
    }

    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        // Writing hex digits into a String cannot fail.
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_inputs_produce_the_same_fingerprint() {
        let a = fingerprint("create_subscription", &["cus_x", "price_y"]);
        let b = fingerprint("create_subscription", &["cus_x", "price_y"]);
        assert_eq!(a, b);
    }

    #[test]
    fn a_changed_input_produces_a_different_fingerprint() {
        let baseline = fingerprint("create_subscription", &["cus_x", "price_y"]);

        assert_ne!(
            baseline,
            fingerprint("create_subscription", &["cus_z", "price_y"]),
            "changing the first field must change the fingerprint"
        );
        assert_ne!(
            baseline,
            fingerprint("create_subscription", &["cus_x", "price_z"]),
            "changing the second field must change the fingerprint"
        );
        assert_ne!(
            baseline,
            fingerprint("update_customer", &["cus_x", "price_y"]),
            "changing the operation must change the fingerprint"
        );
    }

    #[test]
    fn length_prefixing_prevents_delimiter_injection() {
        // A `|`-joined encoding would make these collide: "a|b","c" joins to
        // "a|b|c", identical to "a","b|c" joined the same way. Length
        // prefixing must keep the two field lists distinct.
        let first = fingerprint("op", &["a|b", "c"]);
        let second = fingerprint("op", &["a", "b|c"]);
        assert_ne!(first, second);
    }

    #[test]
    fn output_is_64_lowercase_hex_characters() {
        let hash = fingerprint("create_customer", &["cus_x"]);
        assert_eq!(hash.len(), 64);
        assert!(
            hash.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }
}
