use std::net::{IpAddr, SocketAddr};

use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;
use identity_domain::ClientIp;

use crate::correlation::correlation_id;
use crate::error::{ErrorKind, IdentityError};
use crate::state::IdentityState;

/// The client address a request is rate-limited as.
///
/// The socket peer, unless the peer is the one configured trusted proxy --
/// then the last address in `X-Forwarded-For`, the one that proxy appended.
/// `X-Forwarded-For` from anyone else is ignored: it is a header the caller
/// writes, and trusting it would let every request claim a fresh address.
/// The same reasoning as never accepting a caller's correlation id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientAddress(pub ClientIp);

impl<S> FromRequestParts<S> for ClientAddress
where
    S: Send + Sync,
{
    type Rejection = IdentityError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let internal = |reason: &str| {
            IdentityError::new(
                ErrorKind::Internal(reason.to_string()),
                correlation_id(parts),
            )
        };
        // Both are wiring, not caller input: a host that serves without
        // connect info, or forgot the state, gets a loud 500 rather than every
        // client silently sharing one rate-limit bucket.
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(addr)| addr.ip())
            .ok_or_else(|| internal("ConnectInfo<SocketAddr> is not installed"))?;
        let trusted_proxy = parts
            .extensions
            .get::<IdentityState>()
            .ok_or_else(|| internal("IdentityState extension is not installed"))?
            .trusted_proxy();

        Ok(Self(ClientIp::new(resolve(
            peer,
            trusted_proxy,
            parts
                .headers
                .get_all("x-forwarded-for")
                .iter()
                .filter_map(|value| value.to_str().ok()),
        ))))
    }
}

/// The client address, given the peer, the trusted proxy and every
/// `X-Forwarded-For` value in order.
fn resolve<'a>(
    peer: IpAddr,
    trusted_proxy: Option<IpAddr>,
    forwarded_for: impl Iterator<Item = &'a str>,
) -> IpAddr {
    if trusted_proxy != Some(peer) {
        return peer;
    }
    forwarded_for
        .flat_map(|value| value.split(','))
        .last()
        .and_then(|last| last.trim().parse().ok())
        .unwrap_or(peer)
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;

    const PROXY: IpAddr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
    const CLIENT: IpAddr = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7));

    #[test]
    fn forwarded_for_from_an_untrusted_peer_is_ignored() {
        assert_eq!(
            resolve(CLIENT, Some(PROXY), ["198.51.100.9"].into_iter()),
            CLIENT
        );
        assert_eq!(resolve(CLIENT, None, ["198.51.100.9"].into_iter()), CLIENT);
    }

    #[test]
    fn the_trusted_proxys_appended_address_is_used() {
        // A client can put anything in the header before it reaches the
        // proxy; only the entry the proxy itself appended -- the last -- is
        // believed.
        assert_eq!(
            resolve(PROXY, Some(PROXY), ["1.2.3.4, 203.0.113.7"].into_iter()),
            CLIENT
        );
        assert_eq!(
            resolve(PROXY, Some(PROXY), ["1.2.3.4", "203.0.113.7"].into_iter()),
            CLIENT
        );
    }

    #[test]
    fn a_trusted_proxy_without_a_usable_header_is_the_client() {
        assert_eq!(resolve(PROXY, Some(PROXY), std::iter::empty()), PROXY);
        assert_eq!(
            resolve(PROXY, Some(PROXY), ["not an ip"].into_iter()),
            PROXY
        );
    }
}
