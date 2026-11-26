//! Rate limiting for log sources that termx does not control.
//!
//! The frontend forwards its errors to the log. A component that fails on
//! every frame would otherwise write thousands of records per second.

use std::time::{Duration, Instant};

/// A token bucket: `burst` records may pass at once, and one more is allowed
/// every `interval` after that.
#[derive(Debug)]
pub struct RateLimiter {
    burst: u32,
    interval: Duration,
    tokens: u32,
    /// Start of the interval that is currently filling up.
    refilled_at: Option<Instant>,
    suppressed: u64,
}

/// Outcome of asking a [`RateLimiter`] whether a record may be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Write the record. `suppressed` is the number of records that were
    /// denied since the last allowed one, so the gap can be reported.
    Allowed {
        /// Records denied since the previous allowed record.
        suppressed: u64,
    },
    /// Drop the record.
    Denied,
}

impl RateLimiter {
    /// Creates a limiter that starts with a full bucket.
    ///
    /// A `burst` of zero is treated as one, and a zero `interval` disables
    /// the limit.
    #[must_use]
    pub fn new(burst: u32, interval: Duration) -> Self {
        let burst = burst.max(1);
        Self {
            burst,
            interval,
            tokens: burst,
            refilled_at: None,
            suppressed: 0,
        }
    }

    /// Asks for permission to write one record now.
    pub fn check(&mut self) -> Decision {
        self.check_at(Instant::now())
    }

    /// Asks for permission to write one record at the given time. Time must
    /// not go backwards between calls.
    pub fn check_at(&mut self, now: Instant) -> Decision {
        self.refill(now);
        if self.tokens == 0 {
            self.suppressed = self.suppressed.saturating_add(1);
            return Decision::Denied;
        }
        self.tokens -= 1;
        Decision::Allowed {
            suppressed: std::mem::take(&mut self.suppressed),
        }
    }

    fn refill(&mut self, now: Instant) {
        if self.interval.is_zero() {
            self.tokens = self.burst;
            return;
        }
        let Some(refilled_at) = self.refilled_at else {
            self.refilled_at = Some(now);
            return;
        };
        let elapsed = now.saturating_duration_since(refilled_at);
        let earned = elapsed.as_nanos() / self.interval.as_nanos();
        if earned == 0 {
            return;
        }
        let missing = self.burst - self.tokens;
        match u32::try_from(earned) {
            Ok(earned) if earned < missing => {
                self.tokens += earned;
                // Keep the remainder of the current interval.
                self.refilled_at = Some(refilled_at + self.interval * earned);
            }
            _ => {
                self.tokens = self.burst;
                self.refilled_at = Some(now);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{Decision, RateLimiter};

    const SECOND: Duration = Duration::from_secs(1);
    const ALLOWED: Decision = Decision::Allowed { suppressed: 0 };

    #[test]
    fn a_burst_passes_and_the_rest_is_denied() {
        let start = Instant::now();
        let mut limiter = RateLimiter::new(3, SECOND);

        assert_eq!(limiter.check_at(start), ALLOWED);
        assert_eq!(limiter.check_at(start), ALLOWED);
        assert_eq!(limiter.check_at(start), ALLOWED);
        assert_eq!(limiter.check_at(start), Decision::Denied);
        assert_eq!(limiter.check_at(start + SECOND / 2), Decision::Denied);
    }

    #[test]
    fn one_record_is_allowed_per_interval_and_reports_what_was_dropped() {
        let start = Instant::now();
        let mut limiter = RateLimiter::new(2, SECOND);
        limiter.check_at(start);
        limiter.check_at(start);
        for _ in 0..5 {
            assert_eq!(limiter.check_at(start), Decision::Denied);
        }

        assert_eq!(
            limiter.check_at(start + SECOND),
            Decision::Allowed { suppressed: 5 }
        );
        assert_eq!(limiter.check_at(start + SECOND), Decision::Denied);
        assert_eq!(
            limiter.check_at(start + SECOND * 2),
            Decision::Allowed { suppressed: 1 }
        );
    }

    #[test]
    fn an_error_loop_is_bounded_over_time() {
        let start = Instant::now();
        let mut limiter = RateLimiter::new(20, SECOND);

        // One record every millisecond for a minute.
        let allowed = (0..60_000u64)
            .filter(|ms| limiter.check_at(start + Duration::from_millis(*ms)) != Decision::Denied)
            .count();

        // The burst, plus one per second.
        assert!(
            (20..=20 + 60).contains(&allowed),
            "allowed {allowed} records"
        );
    }

    #[test]
    fn the_bucket_never_holds_more_than_the_burst() {
        let start = Instant::now();
        let mut limiter = RateLimiter::new(2, SECOND);
        limiter.check_at(start);

        let later = start + SECOND * 3600;
        assert_eq!(limiter.check_at(later), ALLOWED);
        assert_eq!(limiter.check_at(later), ALLOWED);
        assert_eq!(limiter.check_at(later), Decision::Denied);
    }

    #[test]
    fn a_zero_interval_disables_the_limit() {
        let start = Instant::now();
        let mut limiter = RateLimiter::new(1, Duration::ZERO);

        for _ in 0..100 {
            assert_eq!(limiter.check_at(start), ALLOWED);
        }
    }

    #[test]
    fn a_zero_burst_still_lets_one_record_through() {
        let start = Instant::now();
        let mut limiter = RateLimiter::new(0, SECOND);

        assert_eq!(limiter.check_at(start), ALLOWED);
        assert_eq!(limiter.check_at(start), Decision::Denied);
    }
}
