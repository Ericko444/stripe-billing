use identity_domain::Clock;
use time::OffsetDateTime;

/// The real clock: `OffsetDateTime::now_utc()`.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
}
