//! Admission control for records from a source termx does not control.
//!
//! The frontend forwards its errors to the log, and a component that fails
//! on every frame would otherwise write thousands of records per second. A
//! [`RecordGate`] bounds that without hiding what matters:
//!
//! - **Every severity has its own budget**, so a flood of low-level records
//!   cannot use up the room for errors.
//! - **The first occurrence of an error is not dropped in favour of
//!   repeats.** An error that was not seen before draws on a second budget
//!   when the error budget is used up. Two errors are the same when they
//!   have the same source and the same [message key](super::message_key),
//!   so numbering the messages does not make each of them new.
//! - **That second budget is per source**, so one source that produces an
//!   endless stream of different errors only exhausts its own, and a new
//!   error from elsewhere still gets through.
//! - **Every dropped record is counted**, per severity, and the counts are
//!   handed out with the next admitted record so that they can be logged.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, RandomState};
use std::time::{Duration, Instant};

use super::normalise::message_key;
use super::rate_limit::{Decision, RateLimiter};

/// Shortest time between two reports of dropped records. During a flood the
/// counts add up and are reported together, instead of in a record of their
/// own for every record that gets through.
pub const REPORT_INTERVAL: Duration = Duration::from_secs(5);

/// Sources that get a budget of their own. Any others share one.
const MAX_SOURCES: usize = 32;

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
    /// Extra budget each source has for errors that were not seen before,
    /// used when `errors` is exhausted.
    pub new_errors_per_source: Budget,
}

impl Default for GateConfig {
    /// 20 errors, 10 warnings and 10 other records at once, then one per
    /// second, per two seconds and per two seconds; and for every source 10
    /// more errors at once, then one per second, for errors not seen before.
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
            new_errors_per_source: Budget {
                burst: 10,
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
    new_errors_budget: Budget,
    /// The budget for new errors of each source, and one for the sources
    /// that came after the first [`MAX_SOURCES`].
    new_errors: HashMap<String, RateLimiter>,
    new_errors_of_other_sources: RateLimiter,
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
            new_errors_budget: config.new_errors_per_source,
            new_errors: HashMap::new(),
            new_errors_of_other_sources: limiter(config.new_errors_per_source),
            seen_errors: HashSet::new(),
            hasher: RandomState::new(),
            dropped: Dropped::default(),
            last_report: None,
        }
    }

    /// Asks whether a record may be written now.
    ///
    /// `source` says where the record comes from and `message` what it is
    /// about. Together they tell a new error from a repeat.
    ///
    /// Returns `None` when the record must be dropped. Otherwise returns the
    /// dropped records the caller should log now: usually none, and at most
    /// once per [`REPORT_INTERVAL`] the counts since the last report, which
    /// are then reset.
    pub fn admit(&mut self, severity: Severity, source: &str, message: &str) -> Option<Dropped> {
        self.admit_at(severity, source, message, Instant::now())
    }

    /// [`RecordGate::admit`] at a given time. Time must not go backwards
    /// between calls.
    pub fn admit_at(
        &mut self,
        severity: Severity,
        source: &str,
        message: &str,
        now: Instant,
    ) -> Option<Dropped> {
        let allowed = |limiter: &mut RateLimiter| limiter.check_at(now) != Decision::Denied;
        let admitted = match severity {
            Severity::Warn => allowed(&mut self.warnings),
            Severity::Info => allowed(&mut self.infos),
            Severity::Error => {
                if self.seen_errors.len() >= MAX_REMEMBERED_ERRORS {
                    self.seen_errors.clear();
                }
                let key = self.hasher.hash_one((source, message_key(message)));
                let new = self.seen_errors.insert(key);
                // A new error falls back on the budget of its source; a
                // repeat cannot.
                allowed(&mut self.errors) || (new && allowed(self.new_errors_of(source)))
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

    /// The budget for new errors of `source`.
    fn new_errors_of(&mut self, source: &str) -> &mut RateLimiter {
        if !self.new_errors.contains_key(source) && self.new_errors.len() >= MAX_SOURCES {
            return &mut self.new_errors_of_other_sources;
        }
        let budget = self.new_errors_budget;
        self.new_errors
            .entry(source.to_owned())
            .or_insert_with(|| RateLimiter::new(budget.burst, budget.interval))
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
            new_errors_per_source: budget(new_errors),
        })
    }

    #[test]
    fn a_flood_of_low_severity_records_does_not_use_up_the_error_budget() {
        let now = Instant::now();
        let mut gate = gate(2, 0, 3);

        let infos = (0..1_000)
            .filter(|_| gate.admit_at(Severity::Info, "ui", "noise", now).is_some())
            .count();
        let warnings = (0..1_000)
            .filter(|_| gate.admit_at(Severity::Warn, "ui", "noise", now).is_some())
            .count();

        assert_eq!((infos, warnings), (3, 3));
        assert!(
            gate.admit_at(Severity::Error, "ui", "real problem", now)
                .is_some()
        );
    }

    #[test]
    fn the_first_occurrence_of_an_error_gets_through_a_flood_of_repeats() {
        let now = Instant::now();
        let mut gate = gate(5, 10, 1);

        let repeats = (0..1_000)
            .filter(|_| {
                gate.admit_at(Severity::Error, "ui", "render loop", now)
                    .is_some()
            })
            .count();
        assert_eq!(repeats, 5, "repeats only have the error budget");

        // The error budget is gone, and a different error still gets in.
        let dropped = gate.admit_at(Severity::Error, "ui", "disk full", now);
        assert_eq!(
            dropped,
            Some(Dropped {
                errors: 995,
                ..Dropped::default()
            })
        );
        // Its own repeats do not.
        assert_eq!(gate.admit_at(Severity::Error, "ui", "disk full", now), None);
    }

    /// A message that stays different after numbers are ignored.
    fn unique(index: u32) -> String {
        let letters: String = index
            .to_string()
            .bytes()
            .map(|digit| char::from(b'a' + digit - b'0'))
            .collect();
        format!("failure {letters}")
    }

    #[test]
    fn numbered_variants_of_one_error_are_repeats_not_new_errors() {
        let now = Instant::now();
        let mut gate = gate(5, 10, 1);

        let admitted = (0..10_000)
            .filter(|index| {
                let message = format!("request {index} failed for session 7f3a{index:04x}");
                gate.admit_at(Severity::Error, "loop", &message, now)
                    .is_some()
            })
            .count();
        assert_eq!(admitted, 5, "the variants only have the error budget");

        // The budget for new errors is untouched: a real one gets through,
        // even from the same source.
        assert!(
            gate.admit_at(Severity::Error, "loop", "the disk is full", now)
                .is_some()
        );
    }

    #[test]
    fn a_source_that_floods_with_different_errors_only_exhausts_its_own_budget() {
        let now = Instant::now();
        let mut gate = gate(5, 10, 1);

        let admitted = (0..10_000)
            .filter(|index| {
                gate.admit_at(Severity::Error, "loop", &unique(*index), now)
                    .is_some()
            })
            .count();
        assert_eq!(
            admitted, 15,
            "the error budget plus the budget of the source"
        );
        assert!(
            gate.admit_at(Severity::Error, "loop", "one more", now)
                .is_none()
        );

        // Another source has its own, and its record brings the count.
        let dropped = gate
            .admit_at(Severity::Error, "disk", "the disk is full", now)
            .unwrap();
        assert_eq!(dropped.errors, 9_986);
    }

    #[test]
    fn the_number_of_sources_with_a_budget_is_bounded() {
        let now = Instant::now();
        let mut gate = gate(1, 2, 1);
        gate.admit_at(Severity::Error, "first", "uses up the error budget", now);

        let admitted = (0..1_000u32)
            .filter(|index| {
                gate.admit_at(Severity::Error, &unique(*index), &unique(*index), now)
                    .is_some()
            })
            .count();

        // One from each of the sources that fit, which is all they sent, and
        // the two that all the other sources share.
        assert_eq!(admitted, super::MAX_SOURCES + 2);
    }

    #[test]
    fn dropped_records_are_counted_per_severity_and_reported_once() {
        let now = Instant::now();
        let mut gate = gate(1, 0, 1);
        assert_eq!(
            gate.admit_at(Severity::Error, "ui", "e", now),
            Some(Dropped::default())
        );
        assert_eq!(
            gate.admit_at(Severity::Warn, "ui", "w", now),
            Some(Dropped::default())
        );
        assert_eq!(
            gate.admit_at(Severity::Info, "ui", "i", now),
            Some(Dropped::default())
        );

        for _ in 0..3 {
            assert_eq!(gate.admit_at(Severity::Error, "ui", "e", now), None);
        }
        for _ in 0..4 {
            assert_eq!(gate.admit_at(Severity::Warn, "ui", "w", now), None);
        }
        for _ in 0..5 {
            assert_eq!(gate.admit_at(Severity::Info, "ui", "i", now), None);
        }

        // The pressure ends: the next admitted record carries the counts.
        let later = now + SECOND;
        let dropped = gate.admit_at(Severity::Info, "ui", "i", later).unwrap();
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
            gate.admit_at(Severity::Error, "ui", "e", later)
                .unwrap()
                .is_empty()
        );
        assert!(gate.take_dropped().is_empty());
    }

    #[test]
    fn budgets_refill_with_time() {
        let now = Instant::now();
        let mut gate = gate(1, 0, 1);
        assert!(gate.admit_at(Severity::Error, "ui", "e", now).is_some());
        assert!(gate.admit_at(Severity::Error, "ui", "e", now).is_none());

        assert!(
            gate.admit_at(Severity::Error, "ui", "e", now + SECOND)
                .is_some()
        );
    }

    #[test]
    fn drops_are_reported_together_at_most_once_per_interval() {
        let now = Instant::now();
        let mut gate = gate(1, 0, 1);
        gate.admit_at(Severity::Info, "ui", "i", now);
        gate.admit_at(Severity::Info, "ui", "i", now);
        let first = gate
            .admit_at(Severity::Info, "ui", "i", now + SECOND)
            .unwrap();
        assert_eq!(first.infos, 1);

        // More drops right after a report wait for the interval to pass.
        gate.admit_at(Severity::Info, "ui", "i", now + SECOND);
        let soon = gate
            .admit_at(Severity::Info, "ui", "i", now + 2 * SECOND)
            .unwrap();
        assert!(soon.is_empty());
        gate.admit_at(Severity::Info, "ui", "i", now + 2 * SECOND);

        let due = now + SECOND + super::REPORT_INTERVAL;
        assert_eq!(
            gate.admit_at(Severity::Info, "ui", "i", due).unwrap().infos,
            2
        );
    }
}
