use std::fmt;
use std::net::{IpAddr, Ipv6Addr};

use sha2::{Digest, Sha256};
use time::Duration;

use crate::Email;

/// The address a request is rate-limited as.
///
/// IPv4 addresses are used whole. IPv6 addresses are cut to their **/64**:
/// a single subscriber is routinely handed a whole /64, so keying on the full
/// address would let one client rotate through 2^64 keys and never be
/// limited. IPv4-mapped IPv6 addresses are treated as the IPv4 address they
/// carry.
///
/// Never written to the audit journal -- an IP address is personal data, and
/// the journal's erasure argument depends on it holding only opaque ids. It
/// lives in the limiter, and in logs that are rotated out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClientIp(IpAddr);

impl ClientIp {
    /// The key for `addr`, normalised as described on the type.
    pub fn new(addr: IpAddr) -> Self {
        match addr {
            IpAddr::V4(v4) => Self(IpAddr::V4(v4)),
            IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
                Some(v4) => Self(IpAddr::V4(v4)),
                None => {
                    let prefix = u128::from(v6) & (u128::MAX << 64);
                    Self(IpAddr::V6(Ipv6Addr::from(prefix)))
                }
            },
        }
    }

    /// The normalised address.
    pub fn addr(&self) -> IpAddr {
        self.0
    }
}

impl fmt::Display for ClientIp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            IpAddr::V4(v4) => write!(f, "{v4}"),
            IpAddr::V6(v6) => write!(f, "{v6}/64"),
        }
    }
}

/// What is being counted: an operation scope plus an address, an IP, or
/// both.
///
/// An address is keyed by a hash of its normalised form, never the address
/// itself, so the limiter's memory is not a list of who tried to log in --
/// and the key is the same whether or not the address has an account, which
/// is what keeps the limiter from becoming an existence oracle.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RateKey(String);

impl RateKey {
    /// Counts `scope` per address.
    pub fn address(scope: &str, email: &Email) -> Self {
        Self(format!("{scope}:address:{}", address_hash(email)))
    }

    /// Counts `scope` per client IP.
    pub fn ip(scope: &str, ip: ClientIp) -> Self {
        Self(format!("{scope}:ip:{ip}"))
    }

    /// Counts `scope` per (address, client IP) pair.
    pub fn address_and_ip(scope: &str, email: &Email, ip: ClientIp) -> Self {
        Self(format!("{scope}:address-ip:{}:{ip}", address_hash(email)))
    }

    /// The key, for a limiter's map.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn address_hash(email: &Email) -> String {
    hex::encode(Sha256::digest(email.as_str().as_bytes()))
}

/// At most `max` events per `window`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimit {
    /// Events allowed in one window.
    pub max: u32,
    /// The window's length.
    pub window: Duration,
}

/// Whether one more event is allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateDecision {
    /// Allowed, and counted.
    Allowed,
    /// Over the limit. `retry_after` is when the window resets.
    Limited {
        /// Time until the current window ends.
        retry_after: Duration,
    },
}

/// Counts events per key and says whether one more is allowed.
///
/// Synchronous and object-safe: the in-memory implementation needs no I/O,
/// and the HTTP layer holds it as `Arc<dyn RateLimiter>`. A limiter shared
/// between several instances (Postgres, Redis) is a different adapter; it
/// would make this port asynchronous, and that is a known limit of the
/// single-instance demo rather than an oversight.
pub trait RateLimiter: Send + Sync {
    /// Counts one event for `key` and decides whether it is within `limit`.
    fn check(&self, key: &RateKey, limit: RateLimit) -> RateDecision;
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;

    #[test]
    fn ipv6_addresses_in_one_slash_64_share_a_key() {
        let one = ClientIp::new(
            "2001:db8:1:2:aaaa::1"
                .parse()
                .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
        );
        let other = ClientIp::new(
            "2001:db8:1:2:bbbb::9"
                .parse()
                .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
        );
        let next_prefix = ClientIp::new(
            "2001:db8:1:3::1"
                .parse()
                .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
        );

        assert_eq!(one, other);
        assert_ne!(one, next_prefix);
        assert_eq!(one.to_string(), "2001:db8:1:2::/64");
    }

    #[test]
    fn ipv4_is_used_whole_and_mapped_ipv6_is_unwrapped() {
        let v4: IpAddr = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7));
        let mapped: IpAddr = IpAddr::V6(Ipv4Addr::new(203, 0, 113, 7).to_ipv6_mapped());

        assert_eq!(ClientIp::new(v4), ClientIp::new(mapped));
        assert_ne!(
            ClientIp::new(v4),
            ClientIp::new(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 8)))
        );
    }

    #[test]
    fn an_address_key_never_contains_the_address() -> Result<(), crate::EmailError> {
        let email = Email::parse("alice@example.test")?;
        let ip = ClientIp::new(IpAddr::V4(Ipv4Addr::LOCALHOST));

        for key in [
            RateKey::address("login", &email),
            RateKey::address_and_ip("login", &email, ip),
        ] {
            assert!(!key.as_str().contains("alice"), "{}", key.as_str());
        }
        assert_eq!(
            RateKey::address("login", &Email::parse("ALICE@example.test")?),
            RateKey::address("login", &email)
        );
        Ok(())
    }
}
