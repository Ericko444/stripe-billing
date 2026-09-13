use axum::http::{HeaderMap, HeaderValue, header};
use secrecy::{ExposeSecret, SecretString};
use time::Duration;

/// The session cookie's name.
///
/// The `__Host-` prefix is enforced by browsers, not by this crate: a cookie
/// with it is only accepted if it is `Secure`, has `Path=/`, and has **no**
/// `Domain` attribute. That last rule is the point -- a sibling subdomain
/// cannot set or shadow a `__Host-` cookie, so a compromised
/// `something.example.com` cannot plant a session on `app.example.com`.
pub const SESSION_COOKIE: &str = "__Host-session";

/// The attributes every session cookie carries.
///
/// - `HttpOnly`: script cannot read it, so an XSS bug cannot exfiltrate it.
/// - `Secure`: never sent over plain HTTP (browsers treat `localhost` as a
///   secure context, so this holds in the demo too).
/// - `SameSite=Strict`: never sent on a request initiated by another site --
///   the first CSRF layer; the origin check is the second.
/// - `Path=/`, no `Domain`: required by the `__Host-` prefix.
const ATTRIBUTES: &str = "HttpOnly; Secure; SameSite=Strict; Path=/";

/// A `Set-Cookie` value carrying `token`, expiring after `max_age`.
///
/// Marked sensitive, so HTTP/2 header compression never indexes it and
/// `Debug` output of the header map does not print it.
pub fn session_cookie(token: &SecretString, max_age: Duration) -> Option<HeaderValue> {
    let seconds = max_age.whole_seconds().max(0);
    let mut value = HeaderValue::from_str(&format!(
        "{SESSION_COOKIE}={}; {ATTRIBUTES}; Max-Age={seconds}",
        token.expose_secret()
    ))
    .ok()?;
    value.set_sensitive(true);
    Some(value)
}

/// A `Set-Cookie` value that removes the session cookie.
pub fn clearing_cookie() -> HeaderValue {
    HeaderValue::from_static(
        "__Host-session=; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age=0",
    )
}

/// The session token from the request's `Cookie` headers, if present and
/// non-empty.
///
/// Browsers send one `Cookie` header, but HTTP/2 allows several; all are
/// read. A pair without `=` is skipped rather than failing the whole header:
/// a malformed cookie set by some other application on the same host must
/// not lock a user out of this one.
pub fn session_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == SESSION_COOKIE)
        .map(|(_, value)| value)
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers_with(cookies: &[&'static str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for cookie in cookies {
            headers.append(header::COOKIE, HeaderValue::from_static(cookie));
        }
        headers
    }

    #[test]
    fn the_session_cookie_has_exactly_the_documented_attributes() {
        let token = SecretString::from("aa.bb");
        let value = session_cookie(&token, Duration::hours(8));

        assert_eq!(
            value.as_ref().and_then(|v| v.to_str().ok()),
            Some("__Host-session=aa.bb; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age=28800")
        );
        assert!(value.is_some_and(|v| v.is_sensitive()));
    }

    #[test]
    fn the_session_cookie_never_has_a_domain() {
        let token = SecretString::from("aa.bb");
        let rendered = session_cookie(&token, Duration::minutes(10))
            .and_then(|v| v.to_str().ok().map(str::to_ascii_lowercase))
            .unwrap_or_default();
        assert!(!rendered.contains("domain"));
    }

    #[test]
    fn the_clearing_cookie_expires_immediately_with_the_same_attributes() {
        assert_eq!(
            clearing_cookie().to_str().ok(),
            Some("__Host-session=; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age=0")
        );
    }

    #[test]
    fn a_negative_max_age_becomes_zero() {
        let value = session_cookie(&SecretString::from("aa.bb"), Duration::seconds(-5));
        assert!(
            value
                .and_then(|v| v.to_str().ok().map(|s| s.ends_with("Max-Age=0")))
                .unwrap_or(false)
        );
    }

    #[test]
    fn finds_the_session_among_other_cookies_and_headers() {
        let headers = headers_with(&["theme=dark; lang=fr", "junk; __Host-session=aa.bb; x=1"]);
        assert_eq!(session_token(&headers), Some("aa.bb"));
    }

    #[test]
    fn a_lookalike_name_or_an_empty_value_is_no_session() {
        assert_eq!(session_token(&headers_with(&["session=aa.bb"])), None);
        assert_eq!(session_token(&headers_with(&["__Host-session="])), None);
        assert_eq!(session_token(&HeaderMap::new()), None);
    }
}
