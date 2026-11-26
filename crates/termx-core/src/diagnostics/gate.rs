//! Admission control for records from a source termx does not control.
//!
//! The frontend forwards its errors to the log, and a component that fails
//! on every frame would otherwise write thousands of records per second. A
//! [`RecordGate`] bounds that without hiding what matters:
//!
//! - **Every severity has its own budget**, so a flood of low-level records
//!   cannot use up the room for errors.
//! - **The first occurrence of an error is not dropped in favour of
//!   repeats.** An error whose key was not seen before draws on a second
//!   budget when the error budget is used up by repeats.
//! - **Every dropped record is counted**, per severity, and the counts are
//!   handed out with the next admitted record so that they can be logged.

use std::collections::HashSet;
use std::hash::{BuildHasher, RandomState};
use std::time::{Duration, Instant};

use super::rate_limit::{Decision, RateLimiter};

/// Shortest time between two reports of dropped records. During a flood the
/// counts add up and are reported together, instead of in a record of their
/// own for every record that gets through.
pub const REPORT_INTERVAL: Duration = Duration::from_secs(5);

/// Distinct error keys remembered before the memory starts over.
const MAX_REMEMBERED_ERRORS: usize = 1_024;

/// How bad the thing a record describes is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Severity {
    /// Something failed.
    Error,
    /// Something unexpected that was recovered from.
    Warn,
    /// Anything less than that.
    Info,
}

/// A burst size and the interval at which one more record is allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    /// Records allowed at once.
    pub burst: u32,
    /// Time after which one more record is allowed.
    pub interval: Duration,
}

/// The budgets of a [`RecordGate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GateConfig {
    /// Budget for errors.
    pub errors: Budget,
    /// Budget for warnings.
    pub warnings: Budget,
    /// Budget for everything else.
    pub infos: Budget,
    /// Extra budget for errors that were not seen before, used when
    /// `errors` is exhausted.
    pub new_errors: Budget,
}

impl Default for GateConfig {
    /// 20 errors, 10 warnings and 10 other records at once, then one per
    /// second, per two seconds and per two seconds; and 50 more errors at
    /// once, then one per second, for errors not seen before.
    fn default() -> Self {
        let second = Duration::from_secs(1);
        Self {
            errors: Budget {
                burst: 20,
                interval: second,
            },
            warnings: Budget {
                burst: 10,
                interval: 2 * second,
            },
            infos: Budget {
                burst: 10,
                interval: 2 * second,
            },
            new_errors: Budget {
                burst: 50,
                interval: second,
            },
        }
    }
}

/// Records dropped by a [`RecordGate`], per severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Dropped {
    /// Dropped errors. These were all repeats of errors already logged,
    /// unless more distinct errors arrived than the gate allows.
    pub errors: u64,
    /// Dropped warnings.
    pub warnings: u64,
    /// Dropped records of lower severity.
    pub infos: u64,
}

impl Dropped {
    /// The number of dropped records of all severities.
    #[must_use]
    pub const fn total(&self) -> u64 {
        self.errors
            .saturating_add(self.warnings)
            .saturating_add(self.infos)
    }

    /// Whether nothing was dropped.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.total() == 0
    }
}

/// Decides which records of an untrusted source are written.
#[derive(Debug)]
pub struct RecordGate {
    errors: RateLimiter,
    warnings: RateLimiter,
    infos: RateLimiter,
    new_errors: RateLimiter,
    /// Hashes of the error keys seen so far.
    seen_errors: HashSet<u64>,
    hasher: RandomState,
    dropped: Dropped,
    last_report: Option<Instant>,
}

impl RecordGate {
    /// Creates a gate with full budgets.
    #[must_use]
    pub fn new(config: GateConfig) -> Self {
        let limiter = |budget: Budget| RateLimiter::new(budget.burst, budget.interval);
        Self {
            errors: limiter(config.errors),
            warnings: limiter(config.warnings),
            infos: limiter(config.infos),
            new_errors: limiter(config.new_errors),
            seen_errors: HashSet::new(),
            hasher: RandomState::new(),
            dropped: Dropped::default(),
            last_report: None,
        }
    }

    /// Asks whether a record may be written now.
    ///
    /// `key` identifies what the record is about (for an error, its source
    /// and message), to tell a new error from a repeat.
    ///
    /// Returns `None` when the record must be dropped. Otherwise returns the
    /// dropped records the caller should log now: usually none, and at most
    /// once per [`REPORT_INTERVAL`] the counts since the last report, which
    /// are then reset.
    pub fn admit(&mut self, severity: Severity, key: &str) -> Option<Dropped> {
        self.admit_at(severity, key, Instant::now())
    }

    /// [`RecordGate::admit`] at a given time. Time must not go backwards
    /// between calls.
    pub fn admit_at(&mut self, severity: Severity, key: &str, now: Instant) -> Option<Dropped> {
        let allowed = |limiter: &mut RateLimiter| limiter.check_at(now) != Decision::Denied;
        let admitted = match severity {
            Severity::Warn => allowed(&mut self.warnings),
            Severity::Info => allowed(&mut self.infos),
            Severity::Error => {
                if self.seen_errors.len() >= MAX_REMEMBERED_ERRORS {
                    self.seen_errors.clear();
                }
                let new = self.seen_errors.insert(self.hasher.hash_one(key));
                // A new error falls back on its own budget; a repeat cannot.
                allowed(&mut self.errors) || (new && allowed(&mut self.new_errors))
            }
        };

        if admitted {
            let report_due = !self.dropped.is_empty()
                && self
                    .last_report
                    .is_none_or(|last| now.saturating_duration_since(last) >= REPORT_INTERVAL);
            if !report_due {
                return Some(Dropped::default());
            }
            self.last_report = Some(now);
            return Some(std::mem::take(&mut self.dropped));
        }
        let counter = match severity {
            Severity::Error => &mut self.dropped.errors,
            Severity::Warn => &mut self.dropped.warnings,
            Severity::Info => &mut self.dropped.infos,
        };
        *counter = counter.saturating_add(1);
        None
    }

    /// Returns the records dropped since they were last handed out, and
    /// resets the counts. For reporting them when nothing else is admitted,
    /// such as at shutdown.
    pub fn take_dropped(&mut self) -> Dropped {
        std::mem::take(&mut self.dropped)
    }
}

impl Default for RecordGate {
    fn default() -> Self {
        Self::new(GateConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{Budget, Dropped, GateConfig, RecordGate, Severity};

    const SECOND: Duration = Duration::from_secs(1);

    fn gate(errors: u32, new_errors: u32, others: u32) -> RecordGate {
        let budget = |burst| Budget {
            burst,
            interval: SECOND,
        };
        RecordGate::new(GateConfig {
            errors: budget(errors),
            warnings: budget(others),
            infos: budget(others),
            new_errors: budget(new_errors),
        })
    }

    #[test]
    fn a_flood_of_low_severity_records_does_not_use_up_the_error_budget() {
        let now = Instant::now();
        let mut gate = gate(2, 0, 3);

        let infos = (0..1_000)
            .filter(|_| gate.admit_at(Severity::Info, "noise", now).is_some())
            .count();
        let warnings = (0..1_000)
            .filter(|_| gate.admit_at(Severity::Warn, "noise", now).is_some())
            .count();

        assert_eq!((infos, warnings), (3, 3));
        assert!(
            gate.admit_at(Severity::Error, "real problem", now)
                .is_some()
        );
    }

    #[test]
    fn the_first_occurrence_of_an_error_gets_through_a_flood_of_repeats() {
        let now = Instant::now();
        let mut gate = gate(5, 10, 1);

        let repeats = (0..1_000)
            .filter(|_| gate.admit_at(Severity::Error, "render loop", now).is_some())
            .count();
        assert_eq!(repeats, 5, "repeats only have the error budget");

        // The error budget is gone, and a different error still gets in.
        let dropped = gate.admit_at(Severity::Error, "disk full", now);
        assert_eq!(
            dropped,
            Some(Dropped {
                errors: 995,
                ..Dropped::default()
            })
        );
        // Its own repeats do not.
        assert_eq!(gate.admit_at(Severity::Error, "disk full", now), None);
    }

    #[test]
    fn distinct_errors_are_bounded_too() {
        let now = Instant::now();
        let mut gate = gate(5, 10, 1);

        let admitted = (0..10_000)
            .filter(|index| {
                gate.admit_at(Severity::Error, &format!("error {index}"), now)
                    .is_some()
            })
            .count();

        assert_eq!(admitted, 15);
        assert_eq!(gate.take_dropped().errors, 9_985);
    }

    #[test]
    fn dropped_records_are_counted_per_severity_and_reported_once() {
        let now = Instant::now();
        let mut gate = gate(1, 0, 1);
        assert_eq!(
            gate.admit_at(Severity::Error, "e", now),
            Some(Dropped::default())
        );
        assert_eq!(
            gate.admit_at(Severity::Warn, "w", now),
            Some(Dropped::default())
        );
        assert_eq!(
            gate.admit_at(Severity::Info, "i", now),
            Some(Dropped::default())
        );

        for _ in 0..3 {
            assert_eq!(gate.admit_at(Severity::Error, "e", now), None);
        }
        for _ in 0..4 {
            assert_eq!(gate.admit_at(Severity::Warn, "w", now), None);
        }
        for _ in 0..5 {
            assert_eq!(gate.admit_at(Severity::Info, "i", now), None);
        }

        // The pressure ends: the next admitted record carries the counts.
        let later = now + SECOND;
        let dropped = gate.admit_at(Severity::Info, "i", later).unwrap();
        assert_eq!(
            dropped,
            Dropped {
                errors: 3,
                warnings: 4,
                infos: 5
            }
        );
        assert_eq!(dropped.total(), 12);
        // And only that one.
        assert!(
            gate.admit_at(Severity::Error, "e", later)
                .unwrap()
                .is_empty()
        );
        assert!(gate.take_dropped().is_empty());
    }

    #[test]
    fn budgets_refill_with_time() {
        let now = Instant::now();
        let mut gate = gate(1, 0, 1);
        assert!(gate.admit_at(Severity::Error, "e", now).is_some());
        assert!(gate.admit_at(Severity::Error, "e", now).is_none());

        assert!(gate.admit_at(Severity::Error, "e", now + SECOND).is_some());
    }

    #[test]
    fn drops_are_reported_together_at_most_once_per_interval() {
        let now = Instant::now();
        let mut gate = gate(1, 0, 1);
        gate.admit_at(Severity::Info, "i", now);
        gate.admit_at(Severity::Info, "i", now);
        let first = gate.admit_at(Severity::Info, "i", now + SECOND).unwrap();
        assert_eq!(first.infos, 1);

        // More drops right after a report wait for the interval to pass.
        gate.admit_at(Severity::Info, "i", now + SECOND);
        let soon = gate
            .admit_at(Severity::Info, "i", now + 2 * SECOND)
            .unwrap();
        assert!(soon.is_empty());
        gate.admit_at(Severity::Info, "i", now + 2 * SECOND);

        let due = now + SECOND + super::REPORT_INTERVAL;
        assert_eq!(gate.admit_at(Severity::Info, "i", due).unwrap().infos, 2);
    }
}
