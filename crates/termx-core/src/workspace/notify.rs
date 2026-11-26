//! Telling a frontend what changed, without making it poll.
//!
//! A [`ChangeHub`] has any number of [`Subscription`]s. Every change of the
//! workspace is published as a [`ChangeSet`]: which sessions changed or
//! went away, and which other parts changed. It says *what* changed, not
//! the new values: the subscriber reads those from the workspace when it
//! gets round to it, so it always sends the latest state and never a stale
//! one.
//!
//! # Coalescing
//!
//! A subscription does not queue change sets: it holds **one**, into which
//! everything published since it was last read is merged. A subscriber that
//! is slow, or that waits a moment before reading
//! ([`Subscription::recv_coalesced`]), therefore gets one change set for a
//! burst of changes, and the memory a subscription holds is bounded by the
//! number of sessions whatever the rate of changes.

use std::collections::BTreeSet;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

use super::model::SessionKey;
use crate::pty::lock;

/// What changed in a workspace. See the [module documentation](self).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChangeSet {
    /// The revision of the workspace after the last change merged in.
    pub revision: u64,
    /// Sessions that were added or changed.
    pub sessions: BTreeSet<SessionKey>,
    /// Sessions that were removed.
    pub removed: BTreeSet<SessionKey>,
    /// Whether the layout changed.
    pub layout: bool,
    /// Whether the list of projects, or its order, changed.
    pub projects: bool,
    /// Whether the order of the sessions changed.
    pub session_order: bool,
    /// Whether the recent folders changed.
    pub recent_folders: bool,
}

impl ChangeSet {
    /// Whether nothing changed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
            && self.removed.is_empty()
            && !self.layout
            && !self.projects
            && !self.session_order
            && !self.recent_folders
    }

    /// Records that `session` was added or changed.
    pub fn session(&mut self, session: &SessionKey) {
        self.sessions.insert(session.clone());
    }

    /// Adds the changes of `later`, which happened after these.
    pub fn merge(&mut self, later: &Self) {
        self.revision = self.revision.max(later.revision);
        self.sessions.extend(later.sessions.iter().cloned());
        self.removed.extend(later.removed.iter().cloned());
        // A session that is gone has no changes worth telling.
        for removed in &self.removed {
            self.sessions.remove(removed);
        }
        self.layout |= later.layout;
        self.projects |= later.projects;
        self.session_order |= later.session_order;
        self.recent_folders |= later.recent_folders;
    }
}

/// What a subscription holds between two reads.
#[derive(Default)]
struct Pending {
    changes: ChangeSet,
    closed: bool,
}

#[derive(Default)]
struct Slot {
    pending: Mutex<Pending>,
    ready: Condvar,
}

/// Publishes the changes of a workspace to its subscribers.
#[derive(Default)]
pub struct ChangeHub {
    slots: Mutex<Vec<Weak<Slot>>>,
}

impl ChangeHub {
    /// Starts a subscription. It receives every change published from now
    /// on.
    #[must_use]
    pub fn subscribe(&self) -> Subscription {
        let slot = Arc::new(Slot::default());
        let mut slots = lock(&self.slots);
        slots.retain(|slot| slot.strong_count() > 0);
        slots.push(Arc::downgrade(&slot));
        Subscription { slot }
    }

    /// Publishes `changes` to every subscription. Never blocks on a
    /// subscriber. An empty change set is not published.
    pub fn publish(&self, changes: &ChangeSet) {
        if changes.is_empty() {
            return;
        }
        let slots: Vec<Arc<Slot>> = lock(&self.slots).iter().filter_map(Weak::upgrade).collect();
        for slot in slots {
            let mut pending = lock(&slot.pending);
            if !pending.closed {
                pending.changes.merge(changes);
                slot.ready.notify_all();
            }
        }
    }

    /// Ends every subscription: their readers are woken up and told that
    /// nothing more will come.
    pub fn close(&self) {
        let slots: Vec<Arc<Slot>> = lock(&self.slots)
            .drain(..)
            .filter_map(|s| s.upgrade())
            .collect();
        for slot in slots {
            lock(&slot.pending).closed = true;
            slot.ready.notify_all();
        }
    }

    /// The number of subscriptions that are still held.
    #[must_use]
    pub fn subscriber_count(&self) -> usize {
        lock(&self.slots)
            .iter()
            .filter(|slot| slot.strong_count() > 0)
            .count()
    }
}

impl std::fmt::Debug for ChangeHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChangeHub")
            .field("subscribers", &self.subscriber_count())
            .finish()
    }
}

/// One subscriber's view of the changes. Clones share the same view, which
/// lets one thread wait while another [closes](Self::close) it.
#[derive(Clone)]
pub struct Subscription {
    slot: Arc<Slot>,
}

impl Subscription {
    /// Takes the changes published since the last read, without waiting.
    /// `None` if there are none.
    #[must_use]
    pub fn try_recv(&self) -> Option<ChangeSet> {
        let mut pending = lock(&self.slot.pending);
        take(&mut pending)
    }

    /// Waits for changes and takes them. `None` once the subscription is
    /// closed and everything was read.
    #[must_use]
    pub fn recv(&self) -> Option<ChangeSet> {
        let mut pending = lock(&self.slot.pending);
        loop {
            if let Some(changes) = take(&mut pending) {
                return Some(changes);
            }
            if pending.closed {
                return None;
            }
            pending = self
                .slot
                .ready
                .wait(pending)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    /// Like [`recv`](Self::recv), but gives up after `timeout`.
    ///
    /// # Errors
    ///
    /// [`RecvError::Timeout`] when nothing changed in time, and
    /// [`RecvError::Closed`] once the subscription is closed and everything
    /// was read.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<ChangeSet, RecvError> {
        let deadline = Instant::now().checked_add(timeout);
        let mut pending = lock(&self.slot.pending);
        loop {
            if let Some(changes) = take(&mut pending) {
                return Ok(changes);
            }
            if pending.closed {
                return Err(RecvError::Closed);
            }
            let remaining = deadline.map_or(Duration::MAX, |deadline| {
                deadline.saturating_duration_since(Instant::now())
            });
            if remaining.is_zero() {
                return Err(RecvError::Timeout);
            }
            pending = self
                .slot
                .ready
                .wait_timeout(pending, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
    }

    /// Waits for a change, then for `window` more, and takes everything
    /// that was published by then as one change set. This is how a
    /// frontend is kept from being told about every step of a burst.
    /// `None` once the subscription is closed and everything was read.
    #[must_use]
    pub fn recv_coalesced(&self, window: Duration) -> Option<ChangeSet> {
        let mut changes = self.recv()?;
        let deadline = Instant::now().checked_add(window);
        let mut pending = lock(&self.slot.pending);
        loop {
            let remaining = deadline.map_or(Duration::ZERO, |deadline| {
                deadline.saturating_duration_since(Instant::now())
            });
            if remaining.is_zero() || pending.closed {
                break;
            }
            pending = self
                .slot
                .ready
                .wait_timeout(pending, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
        if let Some(more) = take(&mut pending) {
            changes.merge(&more);
        }
        Some(changes)
    }

    /// Ends the subscription: nothing more is delivered to it, and a
    /// reader that is waiting is woken up.
    pub fn close(&self) {
        lock(&self.slot.pending).closed = true;
        self.slot.ready.notify_all();
    }

    /// Whether the subscription was closed, by [`close`](Self::close) or by
    /// its hub.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        lock(&self.slot.pending).closed
    }
}

impl std::fmt::Debug for Subscription {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Subscription")
            .field("closed", &self.is_closed())
            .finish()
    }
}

fn take(pending: &mut Pending) -> Option<ChangeSet> {
    if pending.changes.is_empty() {
        None
    } else {
        Some(std::mem::take(&mut pending.changes))
    }
}

/// Why [`Subscription::recv_timeout`] returned no changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RecvError {
    /// Nothing changed in time.
    #[error("nothing changed in time")]
    Timeout,
    /// The subscription is closed.
    #[error("the subscription is closed")]
    Closed,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(name: &str) -> SessionKey {
        SessionKey::from_raw(name).unwrap()
    }

    fn changed(revision: u64, session: &str) -> ChangeSet {
        let mut changes = ChangeSet {
            revision,
            ..ChangeSet::default()
        };
        changes.session(&key(session));
        changes
    }

    #[test]
    fn a_burst_of_changes_is_received_as_one_change_set() {
        let hub = ChangeHub::default();
        let subscription = hub.subscribe();

        for revision in 1..=1000 {
            hub.publish(&changed(
                revision,
                if revision % 2 == 0 { "s-a" } else { "s-b" },
            ));
        }
        hub.publish(&ChangeSet {
            revision: 1001,
            layout: true,
            ..ChangeSet::default()
        });

        let changes = subscription.try_recv().unwrap();
        assert_eq!(changes.revision, 1001);
        assert_eq!(
            changes.sessions.into_iter().collect::<Vec<_>>(),
            [key("s-a"), key("s-b")]
        );
        assert!(changes.layout);
        assert!(!changes.projects);
        // Everything was taken at once.
        assert_eq!(subscription.try_recv(), None);
    }

    #[test]
    fn a_removed_session_is_not_also_reported_as_changed() {
        let mut changes = changed(1, "s-a");
        changes.session(&key("s-b"));
        let mut removal = ChangeSet {
            revision: 2,
            projects: true,
            ..ChangeSet::default()
        };
        removal.removed.insert(key("s-a"));

        changes.merge(&removal);

        assert_eq!(
            changes.sessions.into_iter().collect::<Vec<_>>(),
            [key("s-b")]
        );
        assert_eq!(
            changes.removed.into_iter().collect::<Vec<_>>(),
            [key("s-a")]
        );
        assert!(changes.projects);
        assert_eq!(changes.revision, 2);
    }

    #[test]
    fn an_empty_change_set_is_not_published() {
        let hub = ChangeHub::default();
        let subscription = hub.subscribe();

        hub.publish(&ChangeSet {
            revision: 5,
            ..ChangeSet::default()
        });

        assert_eq!(subscription.try_recv(), None);
        assert_eq!(
            subscription.recv_timeout(Duration::from_millis(10)),
            Err(RecvError::Timeout)
        );
    }

    #[test]
    fn every_subscriber_gets_the_changes_made_after_it_subscribed() {
        let hub = ChangeHub::default();
        let early = hub.subscribe();
        hub.publish(&changed(1, "s-a"));
        let late = hub.subscribe();
        hub.publish(&changed(2, "s-b"));

        assert_eq!(early.try_recv().unwrap().sessions.len(), 2);
        let seen = late.try_recv().unwrap();
        assert_eq!(seen.sessions.into_iter().collect::<Vec<_>>(), [key("s-b")]);
        assert_eq!(hub.subscriber_count(), 2);

        drop(early);
        assert_eq!(hub.subscriber_count(), 1);
        // Publishing to a subscription that is gone is harmless.
        hub.publish(&changed(3, "s-c"));
        assert_eq!(late.try_recv().unwrap().revision, 3);
    }

    #[test]
    fn a_waiting_reader_is_woken_by_a_change() {
        let hub = Arc::new(ChangeHub::default());
        let subscription = hub.subscribe();
        let reader = std::thread::spawn(move || subscription.recv());

        std::thread::sleep(Duration::from_millis(30));
        hub.publish(&changed(7, "s-a"));

        assert_eq!(reader.join().unwrap().unwrap().revision, 7);
    }

    #[test]
    fn changes_within_the_window_are_coalesced() {
        let hub = Arc::new(ChangeHub::default());
        let subscription = hub.subscribe();
        let writer = std::thread::spawn({
            let hub = Arc::clone(&hub);
            move || {
                for revision in 1..=20 {
                    hub.publish(&changed(revision, &format!("s-{revision}")));
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        });

        // A window far longer than the burst: one change set has it all.
        let changes = subscription.recv_coalesced(Duration::from_secs(3)).unwrap();
        writer.join().unwrap();

        assert_eq!(changes.revision, 20);
        assert_eq!(changes.sessions.len(), 20);
        assert_eq!(subscription.try_recv(), None);
    }

    #[test]
    fn closing_wakes_the_reader_after_what_was_published() {
        let hub = ChangeHub::default();
        let subscription = hub.subscribe();
        hub.publish(&changed(1, "s-a"));

        hub.close();

        assert!(subscription.is_closed());
        // What was published before is still delivered, then the end.
        assert_eq!(subscription.recv().unwrap().revision, 1);
        assert_eq!(subscription.recv(), None);
        assert_eq!(
            subscription.recv_timeout(Duration::from_secs(5)),
            Err(RecvError::Closed)
        );
        assert_eq!(subscription.recv_coalesced(Duration::from_secs(5)), None);
        // Nothing is delivered after the end.
        hub.publish(&changed(2, "s-b"));
        assert_eq!(subscription.try_recv(), None);
        assert_eq!(hub.subscriber_count(), 0);
    }

    #[test]
    fn a_subscription_can_be_closed_from_another_thread() {
        let hub = ChangeHub::default();
        let subscription = hub.subscribe();
        let reader = std::thread::spawn({
            let subscription = subscription.clone();
            move || subscription.recv()
        });

        std::thread::sleep(Duration::from_millis(30));
        subscription.close();

        assert_eq!(reader.join().unwrap(), None);
        hub.publish(&changed(1, "s-a"));
        assert_eq!(subscription.try_recv(), None);
    }

    #[test]
    fn a_closed_subscription_ends_a_coalescing_wait_early() {
        let hub = ChangeHub::default();
        let subscription = hub.subscribe();
        hub.publish(&changed(1, "s-a"));
        let closer = std::thread::spawn({
            let subscription = subscription.clone();
            move || {
                std::thread::sleep(Duration::from_millis(30));
                subscription.close();
            }
        });
        let started = Instant::now();

        let changes = subscription.recv_coalesced(Duration::from_secs(60));

        closer.join().unwrap();
        assert_eq!(changes.unwrap().revision, 1);
        assert!(started.elapsed() < Duration::from_secs(30));
    }
}
