use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

use identity_domain::{Clock, RateDecision, RateKey, RateLimit, RateLimiter};
use time::OffsetDateTime;

/// Above this many tracked keys, expired windows are swept on the next
/// check, so a flood of distinct keys cannot grow the map without bound.
const SWEEP_ABOVE: usize = 10_000;

/// A fixed-window counter per key, in process memory.
///
/// Fixed windows over sliding ones: simpler to read and to test, and the one
/// known weakness -- up to twice the limit across a window boundary -- is
/// immaterial at these limits. Single-instance only: several processes each
/// keep their own counts, which is the named limit of the demo's limiter.
pub struct InMemoryRateLimiter<C> {
    clock: C,
    windows: Mutex<HashMap<String, Window>>,
}

#[derive(Debug, Clone, Copy)]
struct Window {
    started_at: OffsetDateTime,
    length: time::Duration,
    count: u32,
}

impl<C: Clock> InMemoryRateLimiter<C> {
    /// A limiter reading time from `clock`.
    pub fn new(clock: C) -> Self {
        Self {
            clock,
            windows: Mutex::new(HashMap::new()),
        }
    }

    fn windows(&self) -> MutexGuard<'_, HashMap<String, Window>> {
        // A panic while holding the lock leaves counts that are at worst
        // stale; limiting on them is better than refusing every request.
        self.windows
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl<C: Clock> RateLimiter for InMemoryRateLimiter<C> {
    fn check(&self, key: &RateKey, limit: RateLimit) -> RateDecision {
        let now = self.clock.now();
        let mut windows = self.windows();

        if windows.len() > SWEEP_ABOVE {
            // Each window expires by its own length: keys under different limits
            // share this map.
            windows.retain(|_, window| now < window.started_at + window.length);
        }

        let window = windows.entry(key.as_str().to_string()).or_insert(Window {
            started_at: now,
            length: limit.window,
            count: 0,
        });
        if now >= window.started_at + limit.window {
            *window = Window {
                started_at: now,
                length: limit.window,
                count: 0,
            };
        }

        if window.count >= limit.max {
            return RateDecision::Limited {
                retry_after: window.started_at + limit.window - now,
            };
        }
        window.count += 1;
        RateDecision::Allowed
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::Arc;

    use identity_domain::ClientIp;
    use time::Duration;

    use super::*;

    /// A clock a test can move forward.
    #[derive(Clone)]
    struct SteppingClock(Arc<Mutex<OffsetDateTime>>);

    impl SteppingClock {
        fn advance(&self, by: Duration) {
            let mut now = self.0.lock().unwrap_or_else(|p| p.into_inner());
            *now += by;
        }
    }

    impl Clock for SteppingClock {
        fn now(&self) -> OffsetDateTime {
            *self.0.lock().unwrap_or_else(|p| p.into_inner())
        }
    }

    const LIMIT: RateLimit = RateLimit {
        max: 3,
        window: Duration::minutes(15),
    };

    fn limiter() -> (InMemoryRateLimiter<SteppingClock>, SteppingClock) {
        let clock = SteppingClock(Arc::new(Mutex::new(OffsetDateTime::UNIX_EPOCH)));
        (InMemoryRateLimiter::new(clock.clone()), clock)
    }

    fn key(last_octet: u8) -> RateKey {
        RateKey::ip(
            "test",
            ClientIp::new(IpAddr::V4(Ipv4Addr::new(203, 0, 113, last_octet))),
        )
    }

    #[test]
    fn the_event_after_the_limit_is_refused_with_the_time_left() {
        let (limiter, clock) = limiter();
        for _ in 0..3 {
            assert_eq!(limiter.check(&key(1), LIMIT), RateDecision::Allowed);
        }
        clock.advance(Duration::minutes(5));

        assert_eq!(
            limiter.check(&key(1), LIMIT),
            RateDecision::Limited {
                retry_after: Duration::minutes(10)
            }
        );
    }

    #[test]
    fn keys_are_counted_separately() {
        let (limiter, _) = limiter();
        for _ in 0..3 {
            limiter.check(&key(1), LIMIT);
        }
        assert_eq!(limiter.check(&key(2), LIMIT), RateDecision::Allowed);
    }

    #[test]
    fn a_new_window_starts_fresh() {
        let (limiter, clock) = limiter();
        for _ in 0..4 {
            limiter.check(&key(1), LIMIT);
        }
        clock.advance(Duration::minutes(15));
        assert_eq!(limiter.check(&key(1), LIMIT), RateDecision::Allowed);
    }

    #[test]
    fn refused_events_do_not_extend_the_window() {
        let (limiter, clock) = limiter();
        for _ in 0..3 {
            limiter.check(&key(1), LIMIT);
        }
        for _ in 0..100 {
            limiter.check(&key(1), LIMIT);
            clock.advance(Duration::seconds(1));
        }
        clock.advance(Duration::minutes(15) - Duration::seconds(100));
        assert_eq!(limiter.check(&key(1), LIMIT), RateDecision::Allowed);
    }
}
