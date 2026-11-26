//! Suppression of repeated warnings and errors.
//!
//! An error in a loop can be logged a million times in a few seconds. Every
//! one of those is written synchronously and takes room, and together they
//! push the record that started it all out of the log.
//!
//! So repeats are counted instead of written. Two records are repeats of
//! each other when they come from the same place in the code and have the
//! same [message key](super::message_key). The first [`WRITTEN_PER_WINDOW`]
//! of them in a [`WINDOW`] are written as they are. The others only
//! increment a counter, and when the window is over one record says how many
//! there were. The first occurrences are therefore always on disk; what a
//! crash in the middle of a window can lose is the count.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Records with the same origin and message written as they are within one
/// [`WINDOW`].
pub const WRITTEN_PER_WINDOW: u32 = 5;

/// Time over which repeats are counted before they are summarised.
pub const WINDOW: Duration = Duration::from_secs(10);

/// Distinct records tracked at once. Beyond that, new ones are written
/// without suppression.
const MAX_TRACKED: usize = 1_024;

/// Longest message kept for a summary, in bytes.
const MAX_MESSAGE_BYTES: usize = 200;

#[derive(Debug)]
struct Tracked {
    window_start: Instant,
    written: u32,
    suppressed: u64,
    level: &'static str,
    target: String,
    message: String,
}

/// Says how many repeats of a record were not written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Summary {
    pub level: &'static str,
    pub target: String,
    /// The message of the first occurrence.
    pub message: String,
    pub repeated: u64,
}

/// What to do with a record.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Write it, after the summary of an earlier window if there is one.
    Write(Option<Summary>),
    /// Count it and write nothing.
    Suppress,
}

#[derive(Debug, Default)]
pub(crate) struct Suppressor {
    tracked: Mutex<HashMap<u64, Tracked>>,
}

impl Suppressor {
    /// Decides about a record identified by `key`.
    pub fn check(
        &self,
        key: u64,
        level: &'static str,
        target: &str,
        message: &str,
        now: Instant,
    ) -> Verdict {
        let mut tracked = self.lock();
        if let Some(entry) = tracked.get_mut(&key) {
            if now.saturating_duration_since(entry.window_start) < WINDOW {
                if entry.written < WRITTEN_PER_WINDOW {
                    entry.written += 1;
                    return Verdict::Write(None);
                }
                entry.suppressed = entry.suppressed.saturating_add(1);
                return Verdict::Suppress;
            }
            let summary = entry.take_summary();
            entry.window_start = now;
            entry.written = 1;
            return Verdict::Write(summary);
        }

        if tracked.len() >= MAX_TRACKED {
            // Make room by forgetting what is over and has nothing to report.
            tracked.retain(|_, entry| {
                entry.suppressed > 0 || now.saturating_duration_since(entry.window_start) < WINDOW
            });
        }
        if tracked.len() < MAX_TRACKED {
            let mut end = message.len().min(MAX_MESSAGE_BYTES);
            while !message.is_char_boundary(end) {
                end -= 1;
            }
            tracked.insert(
                key,
                Tracked {
                    window_start: now,
                    written: 1,
                    suppressed: 0,
                    level,
                    target: target.to_owned(),
                    message: message[..end].to_owned(),
                },
            );
        }
        Verdict::Write(None)
    }

    /// The summaries of the windows that are over at `now`.
    pub fn take_expired(&self, now: Instant) -> Vec<Summary> {
        self.take(|entry| now.saturating_duration_since(entry.window_start) >= WINDOW)
    }

    /// The summaries of everything counted so far. For when the log is
    /// flushed or the process ends.
    pub fn take_all(&self) -> Vec<Summary> {
        self.take(|_| true)
    }

    fn take(&self, due: impl Fn(&Tracked) -> bool) -> Vec<Summary> {
        let mut tracked = self.lock();
        let mut summaries = Vec::new();
        tracked.retain(|_, entry| {
            if !due(entry) {
                return true;
            }
            summaries.extend(entry.take_summary());
            // Forget it: the next occurrence starts a new window.
            false
        });
        summaries
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<u64, Tracked>> {
        self.tracked.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Tracked {
    fn take_summary(&mut self) -> Option<Summary> {
        let repeated = std::mem::take(&mut self.suppressed);
        (repeated > 0).then(|| Summary {
            level: self.level,
            target: self.target.clone(),
            message: self.message.clone(),
            repeated,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::{MAX_TRACKED, Suppressor, Verdict, WINDOW, WRITTEN_PER_WINDOW};

    fn check(suppressor: &Suppressor, key: u64, now: Instant) -> Verdict {
        suppressor.check(key, "ERROR", "test", "it broke", now)
    }

    #[test]
    fn the_first_occurrences_are_written_and_the_rest_counted() {
        let suppressor = Suppressor::default();
        let now = Instant::now();

        for _ in 0..WRITTEN_PER_WINDOW {
            assert_eq!(check(&suppressor, 1, now), Verdict::Write(None));
        }
        for _ in 0..1_000 {
            assert_eq!(check(&suppressor, 1, now), Verdict::Suppress);
        }

        // Another record is not affected.
        assert_eq!(check(&suppressor, 2, now), Verdict::Write(None));
    }

    #[test]
    fn the_next_window_starts_with_a_summary_of_the_last() {
        let suppressor = Suppressor::default();
        let now = Instant::now();
        for _ in 0..WRITTEN_PER_WINDOW + 37 {
            check(&suppressor, 1, now);
        }

        let Verdict::Write(Some(summary)) = check(&suppressor, 1, now + WINDOW) else {
            panic!("expected a summary");
        };
        assert_eq!(summary.repeated, 37);
        assert_eq!(summary.level, "ERROR");
        assert_eq!(summary.target, "test");
        assert_eq!(summary.message, "it broke");
        // Reported once.
        assert_eq!(check(&suppressor, 1, now + WINDOW), Verdict::Write(None));
    }

    #[test]
    fn summaries_are_available_when_the_window_ends_without_a_new_occurrence() {
        let suppressor = Suppressor::default();
        let now = Instant::now();
        for _ in 0..WRITTEN_PER_WINDOW + 3 {
            check(&suppressor, 1, now);
        }

        assert!(suppressor.take_expired(now + WINDOW / 2).is_empty());
        let summaries = suppressor.take_expired(now + WINDOW);
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].repeated, 3);
        assert!(suppressor.take_all().is_empty());
    }

    #[test]
    fn everything_counted_can_be_taken_at_once() {
        let suppressor = Suppressor::default();
        let now = Instant::now();
        for key in 1..=3 {
            for _ in 0..WRITTEN_PER_WINDOW + key as u32 {
                check(&suppressor, key, now);
            }
        }

        let mut repeated: Vec<u64> = suppressor
            .take_all()
            .iter()
            .map(|summary| summary.repeated)
            .collect();
        repeated.sort_unstable();
        assert_eq!(repeated, [1, 2, 3]);
    }

    #[test]
    fn more_distinct_records_than_can_be_tracked_are_written_unsuppressed() {
        let suppressor = Suppressor::default();
        let now = Instant::now();

        for key in 0..(MAX_TRACKED as u64 * 3) {
            assert_eq!(check(&suppressor, key, now), Verdict::Write(None));
        }
        assert!(suppressor.lock().len() <= MAX_TRACKED);
    }
}
