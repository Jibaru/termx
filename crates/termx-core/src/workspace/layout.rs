//! The layout of the main area: a tree of panes.
//!
//! The tree is binary. A [`Node::Pane`] is a leaf that shows one session, or
//! nothing. A [`Node::Split`] divides its area between exactly two children,
//! side by side ([`Direction::Row`]) or stacked ([`Direction::Column`]),
//! giving `ratio` of it to the first one. A grid is splits within splits.
//!
//! # Invariants
//!
//! Every operation keeps these true, and [`Layout::sanitize`] restores them
//! for a layout read from disk:
//!
//! - There is always at least one pane. Closing the last one empties it.
//! - A split always has two children: closing a pane replaces its parent
//!   split by the sibling. There are no empty splits.
//! - Every ratio is finite and within [`MIN_RATIO`]..=[`MAX_RATIO`].
//! - Pane and split identifiers are unique within the layout and are never
//!   reused. They stay at most [`MAX_ID`]: a layout read from disk with
//!   larger ones is renumbered.
//! - A session is shown in at most one pane.
//! - The focused pane exists.
//! - There are at most [`MAX_PANES`] panes.

use serde::{Deserialize, Serialize};

use super::SessionKey;

/// Largest identifier a layout read from disk may use. A layout of
/// [`MAX_PANES`] panes that is edited for years stays far below it; one
/// above it is renumbered from 1, so that identifiers can never run out.
pub const MAX_ID: u32 = 1 << 24;

/// Smallest share of a split its first child can be given.
pub const MIN_RATIO: f64 = 0.1;

/// Largest share of a split its first child can be given.
pub const MAX_RATIO: f64 = 0.9;

/// Largest number of panes of a layout. Far more than fits on a screen, and
/// it bounds the depth of the tree, which is walked recursively.
pub const MAX_PANES: usize = 32;

/// Identifies a pane within a layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PaneId(u32);

impl PaneId {
    /// Wraps a raw identifier, as received from a frontend.
    #[must_use]
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    /// The raw identifier.
    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self.0
    }
}

impl std::fmt::Display for PaneId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Identifies a split within a layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SplitId(u32);

impl SplitId {
    /// Wraps a raw identifier, as received from a frontend.
    #[must_use]
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    /// The raw identifier.
    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self.0
    }
}

impl std::fmt::Display for SplitId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// How a split arranges its two children.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Direction {
    /// Side by side: the first child on the left, the second on the right.
    Row,
    /// Stacked: the first child on top, the second below.
    Column,
}

/// Where a new pane goes relative to the pane that is split.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Placement {
    /// To the left of it, or above it.
    Before,
    /// To the right of it, or below it.
    #[default]
    After,
}

/// Where a pane that is moved lands relative to its target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Edge {
    /// The target is split side by side and the pane takes its left half.
    Left,
    /// The target is split side by side and the pane takes its right half.
    Right,
    /// The target is split in a stack and the pane takes its top half.
    Top,
    /// The target is split in a stack and the pane takes its bottom half.
    Bottom,
    /// The two panes exchange what they show. The tree does not change.
    Center,
}

/// A node of the layout tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Node {
    /// A leaf: an area that shows one session, or nothing.
    Pane {
        /// Identifier of the pane.
        id: PaneId,
        /// The session shown, if any.
        #[serde(default)]
        session: Option<SessionKey>,
    },
    /// An area divided between two children.
    Split {
        /// Identifier of the split, to resize it.
        id: SplitId,
        /// How the children are arranged.
        direction: Direction,
        /// The share of the area given to `first`, within
        /// [`MIN_RATIO`]..=[`MAX_RATIO`].
        ratio: f64,
        /// The left or top child.
        first: Box<Node>,
        /// The right or bottom child.
        second: Box<Node>,
    },
}

/// Why a layout operation was refused. The layout is unchanged.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum LayoutError {
    /// No pane has this identifier.
    #[error("no pane with id {0}")]
    UnknownPane(PaneId),
    /// No split has this identifier.
    #[error("no split with id {0}")]
    UnknownSplit(SplitId),
    /// The layout already has [`MAX_PANES`] panes.
    #[error("the layout cannot have more than {MAX_PANES} panes")]
    TooManyPanes,
    /// A pane cannot be moved next to itself.
    #[error("a pane cannot be moved onto itself")]
    SamePane,
    /// The ratio is not a finite number.
    #[error("the ratio of a split must be a finite number")]
    InvalidRatio,
}

/// The layout of the main area. See the [module documentation](self).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Layout {
    root: Node,
    focused: PaneId,
    /// The next identifier to hand out, shared by panes and splits.
    next_id: u32,
}

impl Default for Layout {
    /// One empty pane.
    fn default() -> Self {
        Self {
            root: Node::Pane {
                id: PaneId(1),
                session: None,
            },
            focused: PaneId(1),
            next_id: 2,
        }
    }
}

impl Layout {
    /// The root of the tree.
    #[must_use]
    pub fn root(&self) -> &Node {
        &self.root
    }

    /// The pane that has the focus.
    #[must_use]
    pub fn focused(&self) -> PaneId {
        self.focused
    }

    /// The panes and what they show, in reading order (first children
    /// first).
    #[must_use]
    pub fn panes(&self) -> Vec<(PaneId, Option<&SessionKey>)> {
        let mut panes = Vec::new();
        collect_panes(&self.root, &mut panes);
        panes
    }

    /// The number of panes.
    #[must_use]
    pub fn pane_count(&self) -> usize {
        count_panes(&self.root)
    }

    /// The pane that shows `session`, if one does.
    #[must_use]
    pub fn pane_of(&self, session: &SessionKey) -> Option<PaneId> {
        self.panes()
            .into_iter()
            .find(|(_, shown)| *shown == Some(session))
            .map(|(id, _)| id)
    }

    /// What `pane` shows.
    ///
    /// # Errors
    ///
    /// [`LayoutError::UnknownPane`] if there is no such pane.
    pub fn session_of(&self, pane: PaneId) -> Result<Option<&SessionKey>, LayoutError> {
        self.panes()
            .into_iter()
            .find(|(id, _)| *id == pane)
            .map(|(_, session)| session)
            .ok_or(LayoutError::UnknownPane(pane))
    }

    /// Splits `pane` in two and returns the new pane, which shows `session`
    /// and takes the focus. Both halves get the same share.
    ///
    /// A session that was shown in another pane moves to the new one.
    ///
    /// # Errors
    ///
    /// [`LayoutError::UnknownPane`] and [`LayoutError::TooManyPanes`].
    pub fn split(
        &mut self,
        pane: PaneId,
        direction: Direction,
        placement: Placement,
        session: Option<SessionKey>,
    ) -> Result<PaneId, LayoutError> {
        self.require_pane(pane)?;
        if self.pane_count() >= MAX_PANES {
            return Err(LayoutError::TooManyPanes);
        }
        let first = self.take_ids(2)?;
        if let Some(session) = &session {
            self.remove_session(session);
        }
        let (new_pane, split) = (PaneId(first), SplitId(first + 1));
        let leaf = Node::Pane {
            id: new_pane,
            session,
        };
        insert_beside(&mut self.root, pane, leaf, split, direction, placement);
        self.focused = new_pane;
        Ok(new_pane)
    }

    /// Closes `pane`: its sibling takes the area of their split. The last
    /// pane is emptied instead. The session that was shown keeps running:
    /// only the pane goes away.
    ///
    /// If the pane had the focus, it moves to the nearest pane of the
    /// sibling.
    ///
    /// # Errors
    ///
    /// [`LayoutError::UnknownPane`].
    pub fn close(&mut self, pane: PaneId) -> Result<(), LayoutError> {
        self.require_pane(pane)?;
        if let Node::Pane { session, .. } = &mut self.root {
            *session = None;
            return Ok(());
        }
        let (_, heir) = detach(&mut self.root, pane).ok_or(LayoutError::UnknownPane(pane))?;
        if self.focused == pane {
            self.focused = heir;
        }
        Ok(())
    }

    /// Moves `pane` to an edge of `target`, or exchanges what the two show
    /// ([`Edge::Center`]). The moved pane keeps its identifier and takes
    /// the focus.
    ///
    /// # Errors
    ///
    /// [`LayoutError::UnknownPane`] and [`LayoutError::SamePane`].
    pub fn move_pane(
        &mut self,
        pane: PaneId,
        target: PaneId,
        edge: Edge,
    ) -> Result<(), LayoutError> {
        self.require_pane(pane)?;
        self.require_pane(target)?;
        if pane == target {
            return Err(LayoutError::SamePane);
        }
        let (direction, placement) = match edge {
            Edge::Center => {
                let ours = self.session_of(pane)?.cloned();
                let theirs = self.session_of(target)?.cloned();
                set_session(&mut self.root, pane, theirs);
                set_session(&mut self.root, target, ours);
                self.focused = pane;
                return Ok(());
            }
            Edge::Left => (Direction::Row, Placement::Before),
            Edge::Right => (Direction::Row, Placement::After),
            Edge::Top => (Direction::Column, Placement::Before),
            Edge::Bottom => (Direction::Column, Placement::After),
        };
        // Two distinct panes exist, so the root is a split and `pane` has a
        // sibling to leave behind.
        let split = SplitId(self.take_ids(1)?);
        let (leaf, _) = detach(&mut self.root, pane).ok_or(LayoutError::UnknownPane(pane))?;
        insert_beside(&mut self.root, target, leaf, split, direction, placement);
        self.focused = pane;
        Ok(())
    }

    /// Sets the share of `split` given to its first child. The value is
    /// brought within [`MIN_RATIO`]..=[`MAX_RATIO`]; the ratio that was set
    /// is returned.
    ///
    /// # Errors
    ///
    /// [`LayoutError::InvalidRatio`] for a value that is not finite, and
    /// [`LayoutError::UnknownSplit`].
    pub fn resize(&mut self, split: SplitId, ratio: f64) -> Result<f64, LayoutError> {
        if !ratio.is_finite() {
            return Err(LayoutError::InvalidRatio);
        }
        let ratio = ratio.clamp(MIN_RATIO, MAX_RATIO);
        if set_ratio(&mut self.root, split, ratio) {
            Ok(ratio)
        } else {
            Err(LayoutError::UnknownSplit(split))
        }
    }

    /// Gives the focus to `pane`.
    ///
    /// # Errors
    ///
    /// [`LayoutError::UnknownPane`].
    pub fn focus(&mut self, pane: PaneId) -> Result<(), LayoutError> {
        self.require_pane(pane)?;
        self.focused = pane;
        Ok(())
    }

    /// Shows `session` in `pane`, or empties the pane. A session that was
    /// shown in another pane moves: that pane becomes empty.
    ///
    /// # Errors
    ///
    /// [`LayoutError::UnknownPane`].
    pub fn show(&mut self, pane: PaneId, session: Option<SessionKey>) -> Result<(), LayoutError> {
        self.require_pane(pane)?;
        if let Some(session) = &session {
            self.remove_session(session);
        }
        set_session(&mut self.root, pane, session);
        Ok(())
    }

    /// Empties the pane that shows `session`. Returns whether one did.
    pub fn remove_session(&mut self, session: &SessionKey) -> bool {
        match self.pane_of(session) {
            Some(pane) => {
                set_session(&mut self.root, pane, None);
                true
            }
            None => false,
        }
    }

    /// Makes a layout that was read from disk valid: see the
    /// [invariants](self#invariants). `known` tells which sessions exist;
    /// panes showing any other become empty.
    ///
    /// A layout that cannot be repaired sensibly (too many panes) is
    /// replaced by the default one. Returns whether anything was changed.
    pub fn sanitize(&mut self, known: impl Fn(&SessionKey) -> bool) -> bool {
        let before = self.clone();
        if count_panes_bounded(&self.root, MAX_PANES + 1) > MAX_PANES {
            *self = Self::default();
            return true;
        }

        let mut seen_ids = std::collections::HashSet::new();
        let mut seen_sessions = std::collections::HashSet::new();
        let mut next_id = self.next_id;
        max_id(&self.root, &mut next_id);
        repair(
            &mut self.root,
            &mut seen_ids,
            &mut seen_sessions,
            &mut next_id,
            &known,
        );
        self.next_id = next_id;
        if !self.panes().iter().any(|(id, _)| *id == self.focused) {
            self.focused = first_pane(&self.root);
        }
        if self.next_id > MAX_ID {
            self.renumber();
        }
        *self != before
    }

    /// Checks the [invariants](self#invariants), describing the first one
    /// that does not hold. Meant for tests and debug assertions.
    ///
    /// # Errors
    ///
    /// A description of the broken invariant.
    pub fn check_invariants(&self) -> Result<(), String> {
        let mut ids = std::collections::HashSet::new();
        let mut sessions = std::collections::HashSet::new();
        check(&self.root, &mut ids, &mut sessions, self.next_id)?;
        let panes = self.panes();
        if panes.is_empty() || panes.len() > MAX_PANES {
            return Err(format!("{} panes", panes.len()));
        }
        if !panes.iter().any(|(id, _)| *id == self.focused) {
            return Err(format!("the focused pane {} does not exist", self.focused));
        }
        Ok(())
    }

    fn require_pane(&self, pane: PaneId) -> Result<(), LayoutError> {
        if self.panes().iter().any(|(id, _)| *id == pane) {
            Ok(())
        } else {
            Err(LayoutError::UnknownPane(pane))
        }
    }

    /// Hands out `count` new identifiers, the first of which is returned.
    ///
    /// Running out takes billions of operations in one run, since
    /// [`sanitize`](Self::sanitize) keeps a restored layout below
    /// [`MAX_ID`]; should it happen, the operation is refused rather than
    /// an identifier handed out twice.
    fn take_ids(&mut self, count: u32) -> Result<u32, LayoutError> {
        let first = self.next_id;
        self.next_id = first.checked_add(count).ok_or(LayoutError::TooManyPanes)?;
        Ok(first)
    }

    /// Gives every node a new identifier, from 1 in reading order, and
    /// keeps the focus on the same pane.
    fn renumber(&mut self) {
        fn walk(node: &mut Node, next: &mut u32, focused: PaneId, new_focus: &mut PaneId) {
            match node {
                Node::Pane { id, .. } => {
                    if *id == focused {
                        *new_focus = PaneId(*next);
                    }
                    id.0 = *next;
                    *next += 1;
                }
                Node::Split {
                    id, first, second, ..
                } => {
                    id.0 = *next;
                    *next += 1;
                    walk(first, next, focused, new_focus);
                    walk(second, next, focused, new_focus);
                }
            }
        }
        let mut next = 1;
        let mut focused = PaneId(1);
        walk(&mut self.root, &mut next, self.focused, &mut focused);
        self.focused = focused;
        self.next_id = next;
    }
}

fn collect_panes<'a>(node: &'a Node, panes: &mut Vec<(PaneId, Option<&'a SessionKey>)>) {
    match node {
        Node::Pane { id, session } => panes.push((*id, session.as_ref())),
        Node::Split { first, second, .. } => {
            collect_panes(first, panes);
            collect_panes(second, panes);
        }
    }
}

fn count_panes(node: &Node) -> usize {
    match node {
        Node::Pane { .. } => 1,
        Node::Split { first, second, .. } => count_panes(first) + count_panes(second),
    }
}

/// Counts panes, giving up once more than `limit` were found.
fn count_panes_bounded(node: &Node, limit: usize) -> usize {
    match node {
        Node::Pane { .. } => 1,
        Node::Split { first, second, .. } => {
            let count = count_panes_bounded(first, limit);
            if count > limit {
                count
            } else {
                count + count_panes_bounded(second, limit)
            }
        }
    }
}

fn first_pane(node: &Node) -> PaneId {
    match node {
        Node::Pane { id, .. } => *id,
        Node::Split { first, .. } => first_pane(first),
    }
}

fn contains_pane(node: &Node, pane: PaneId) -> bool {
    match node {
        Node::Pane { id, .. } => *id == pane,
        Node::Split { first, second, .. } => {
            contains_pane(first, pane) || contains_pane(second, pane)
        }
    }
}

fn set_session(node: &mut Node, pane: PaneId, value: Option<SessionKey>) {
    match node {
        Node::Pane { id, session } => {
            if *id == pane {
                *session = value;
            }
        }
        Node::Split { first, second, .. } => {
            if contains_pane(first, pane) {
                set_session(first, pane, value);
            } else {
                set_session(second, pane, value);
            }
        }
    }
}

fn set_ratio(node: &mut Node, split: SplitId, value: f64) -> bool {
    match node {
        Node::Pane { .. } => false,
        Node::Split {
            id,
            ratio,
            first,
            second,
            ..
        } => {
            if *id == split {
                *ratio = value;
                true
            } else {
                set_ratio(first, split, value) || set_ratio(second, split, value)
            }
        }
    }
}

/// A stand-in while a node is taken out of the tree to be rebuilt.
fn placeholder() -> Node {
    Node::Pane {
        id: PaneId(0),
        session: None,
    }
}

/// Replaces the pane `target` by a split of it and `leaf`.
fn insert_beside(
    node: &mut Node,
    target: PaneId,
    leaf: Node,
    split: SplitId,
    direction: Direction,
    placement: Placement,
) {
    match node {
        Node::Pane { id, .. } if *id == target => {
            let existing = Box::new(std::mem::replace(node, placeholder()));
            let leaf = Box::new(leaf);
            let (first, second) = match placement {
                Placement::Before => (leaf, existing),
                Placement::After => (existing, leaf),
            };
            *node = Node::Split {
                id: split,
                direction,
                ratio: 0.5,
                first,
                second,
            };
        }
        Node::Pane { .. } => {}
        Node::Split { first, second, .. } => {
            if contains_pane(first, target) {
                insert_beside(first, target, leaf, split, direction, placement);
            } else {
                insert_beside(second, target, leaf, split, direction, placement);
            }
        }
    }
}

/// Takes the pane `pane` out of the tree: its parent split is replaced by
/// its sibling. Returns the pane and the first pane of that sibling.
///
/// Returns `None`, changing nothing, when `node` is the pane itself (it has
/// no parent to collapse) or does not contain it.
fn detach(node: &mut Node, pane: PaneId) -> Option<(Node, PaneId)> {
    let Node::Split { first, second, .. } = node else {
        return None;
    };
    let in_first = matches!(**first, Node::Pane { id, .. } if id == pane);
    let in_second = matches!(**second, Node::Pane { id, .. } if id == pane);
    if in_first || in_second {
        let Node::Split { first, second, .. } = std::mem::replace(node, placeholder()) else {
            return None;
        };
        let (leaf, sibling) = if in_first {
            (*first, *second)
        } else {
            (*second, *first)
        };
        let heir = first_pane(&sibling);
        *node = sibling;
        return Some((leaf, heir));
    }
    if contains_pane(first, pane) {
        detach(first, pane)
    } else {
        detach(second, pane)
    }
}

fn max_id(node: &Node, next_id: &mut u32) {
    match node {
        Node::Pane { id, .. } => *next_id = (*next_id).max(id.0.saturating_add(1)),
        Node::Split {
            id, first, second, ..
        } => {
            *next_id = (*next_id).max(id.0.saturating_add(1));
            max_id(first, next_id);
            max_id(second, next_id);
        }
    }
}

fn repair(
    node: &mut Node,
    seen_ids: &mut std::collections::HashSet<u32>,
    seen_sessions: &mut std::collections::HashSet<SessionKey>,
    next_id: &mut u32,
    known: &impl Fn(&SessionKey) -> bool,
) {
    let mut fresh = |raw: &mut u32| {
        if *raw == 0 || !seen_ids.insert(*raw) {
            *raw = *next_id;
            *next_id = next_id.saturating_add(1);
            seen_ids.insert(*raw);
        }
    };
    match node {
        Node::Pane { id, session } => {
            fresh(&mut id.0);
            if let Some(key) = session
                && (!known(key) || !seen_sessions.insert(key.clone()))
            {
                *session = None;
            }
        }
        Node::Split {
            id,
            ratio,
            first,
            second,
            ..
        } => {
            fresh(&mut id.0);
            *ratio = if ratio.is_finite() {
                ratio.clamp(MIN_RATIO, MAX_RATIO)
            } else {
                0.5
            };
            repair(first, seen_ids, seen_sessions, next_id, known);
            repair(second, seen_ids, seen_sessions, next_id, known);
        }
    }
}

fn check(
    node: &Node,
    ids: &mut std::collections::HashSet<u32>,
    sessions: &mut std::collections::HashSet<SessionKey>,
    next_id: u32,
) -> Result<(), String> {
    let mut unique = |raw: u32| {
        if raw == 0 || raw >= next_id {
            return Err(format!("id {raw} was never handed out"));
        }
        if ids.insert(raw) {
            Ok(())
        } else {
            Err(format!("id {raw} is used twice"))
        }
    };
    match node {
        Node::Pane { id, session } => {
            unique(id.0)?;
            if let Some(session) = session
                && !sessions.insert(session.clone())
            {
                return Err(format!("session {session} is shown twice"));
            }
            Ok(())
        }
        Node::Split {
            id,
            ratio,
            first,
            second,
            ..
        } => {
            unique(id.0)?;
            if !ratio.is_finite() || !(MIN_RATIO..=MAX_RATIO).contains(ratio) {
                return Err(format!("ratio {ratio} of split {id} is out of bounds"));
            }
            check(first, ids, sessions, next_id)?;
            check(second, ids, sessions, next_id)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(name: &str) -> SessionKey {
        SessionKey::from_raw(name).unwrap()
    }

    fn shown(layout: &Layout) -> Vec<Option<String>> {
        layout
            .panes()
            .into_iter()
            .map(|(_, session)| session.map(ToString::to_string))
            .collect()
    }

    fn ids(layout: &Layout) -> Vec<PaneId> {
        layout.panes().into_iter().map(|(id, _)| id).collect()
    }

    #[test]
    fn the_default_layout_is_one_empty_focused_pane() {
        let layout = Layout::default();

        assert_eq!(layout.pane_count(), 1);
        assert_eq!(shown(&layout), [None]);
        assert_eq!(layout.focused(), ids(&layout)[0]);
        layout.check_invariants().unwrap();
    }

    #[test]
    fn splitting_puts_the_new_pane_before_or_after_and_focuses_it() {
        let mut layout = Layout::default();
        let first = layout.focused();
        layout.show(first, Some(key("s-a"))).unwrap();

        let right = layout
            .split(first, Direction::Row, Placement::After, Some(key("s-b")))
            .unwrap();
        let top = layout
            .split(first, Direction::Column, Placement::Before, None)
            .unwrap();

        assert_eq!(ids(&layout), [top, first, right]);
        assert_eq!(
            shown(&layout),
            [None, Some("s-a".to_owned()), Some("s-b".to_owned())]
        );
        assert_eq!(layout.focused(), top);
        let Node::Split {
            direction, ratio, ..
        } = layout.root()
        else {
            panic!("the root is not a split");
        };
        assert_eq!(*direction, Direction::Row);
        assert!((ratio - 0.5).abs() < f64::EPSILON);
        layout.check_invariants().unwrap();
    }

    #[test]
    fn splitting_with_a_session_shown_elsewhere_moves_it() {
        let mut layout = Layout::default();
        let first = layout.focused();
        layout.show(first, Some(key("s-a"))).unwrap();

        let second = layout
            .split(first, Direction::Row, Placement::After, Some(key("s-a")))
            .unwrap();

        assert_eq!(shown(&layout), [None, Some("s-a".to_owned())]);
        assert_eq!(layout.pane_of(&key("s-a")), Some(second));
    }

    #[test]
    fn closing_a_pane_collapses_its_split_and_moves_the_focus() {
        let mut layout = Layout::default();
        let first = layout.focused();
        let second = layout
            .split(first, Direction::Row, Placement::After, Some(key("s-b")))
            .unwrap();
        let third = layout
            .split(second, Direction::Column, Placement::After, None)
            .unwrap();
        assert_eq!(layout.focused(), third);

        layout.close(third).unwrap();

        assert_eq!(ids(&layout), [first, second]);
        assert_eq!(layout.focused(), second);
        layout.close(first).unwrap();
        assert_eq!(ids(&layout), [second]);
        assert!(matches!(layout.root(), Node::Pane { .. }));
        layout.check_invariants().unwrap();
    }

    #[test]
    fn closing_the_last_pane_empties_it() {
        let mut layout = Layout::default();
        let only = layout.focused();
        layout.show(only, Some(key("s-a"))).unwrap();

        layout.close(only).unwrap();

        assert_eq!(ids(&layout), [only]);
        assert_eq!(shown(&layout), [None]);
        assert_eq!(layout.focused(), only);
    }

    #[test]
    fn moving_a_pane_to_an_edge_keeps_its_identity_and_content() {
        let mut layout = Layout::default();
        let a = layout.focused();
        layout.show(a, Some(key("s-a"))).unwrap();
        let b = layout
            .split(a, Direction::Row, Placement::After, Some(key("s-b")))
            .unwrap();
        let c = layout
            .split(b, Direction::Row, Placement::After, Some(key("s-c")))
            .unwrap();

        layout.move_pane(c, a, Edge::Top).unwrap();

        assert_eq!(ids(&layout), [c, a, b]);
        assert_eq!(layout.pane_of(&key("s-c")), Some(c));
        assert_eq!(layout.focused(), c);
        layout.check_invariants().unwrap();

        layout.move_pane(a, b, Edge::Right).unwrap();
        assert_eq!(ids(&layout), [c, b, a]);
        layout.check_invariants().unwrap();
    }

    #[test]
    fn moving_to_the_centre_exchanges_what_two_panes_show() {
        let mut layout = Layout::default();
        let a = layout.focused();
        layout.show(a, Some(key("s-a"))).unwrap();
        let b = layout
            .split(a, Direction::Row, Placement::After, None)
            .unwrap();
        let before = layout.root().clone();

        layout.move_pane(a, b, Edge::Center).unwrap();

        assert_eq!(ids(&layout), [a, b]);
        assert_eq!(shown(&layout), [None, Some("s-a".to_owned())]);
        assert_ne!(*layout.root(), before);
        assert_eq!(layout.focused(), a);
    }

    #[test]
    fn a_pane_cannot_be_moved_onto_itself() {
        let mut layout = Layout::default();
        let a = layout.focused();
        let before = layout.clone();

        assert_eq!(
            layout.move_pane(a, a, Edge::Left),
            Err(LayoutError::SamePane)
        );
        assert_eq!(layout, before);
    }

    #[test]
    fn ratios_are_kept_within_bounds() {
        let mut layout = Layout::default();
        let a = layout.focused();
        layout
            .split(a, Direction::Row, Placement::After, None)
            .unwrap();
        let Node::Split { id: split, .. } = *layout.root() else {
            panic!("the root is not a split");
        };

        assert_eq!(layout.resize(split, 0.3), Ok(0.3));
        assert_eq!(layout.resize(split, 0.0), Ok(MIN_RATIO));
        assert_eq!(layout.resize(split, 7.0), Ok(MAX_RATIO));
        assert_eq!(layout.resize(split, -1.0), Ok(MIN_RATIO));
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(layout.resize(split, bad), Err(LayoutError::InvalidRatio));
        }
        assert_eq!(
            layout.resize(SplitId(99), 0.5),
            Err(LayoutError::UnknownSplit(SplitId(99)))
        );
        layout.check_invariants().unwrap();
    }

    #[test]
    fn showing_a_session_in_another_pane_moves_it() {
        let mut layout = Layout::default();
        let a = layout.focused();
        let b = layout
            .split(a, Direction::Row, Placement::After, None)
            .unwrap();
        layout.show(a, Some(key("s-a"))).unwrap();

        layout.show(b, Some(key("s-a"))).unwrap();

        assert_eq!(shown(&layout), [None, Some("s-a".to_owned())]);
        assert!(layout.remove_session(&key("s-a")));
        assert!(!layout.remove_session(&key("s-a")));
        assert_eq!(shown(&layout), [None, None]);
    }

    #[test]
    fn operations_on_unknown_panes_change_nothing() {
        let mut layout = Layout::default();
        let a = layout.focused();
        layout
            .split(a, Direction::Row, Placement::After, None)
            .unwrap();
        let before = layout.clone();
        let ghost = PaneId(99);
        let unknown = Err(LayoutError::UnknownPane(ghost));

        assert_eq!(
            layout
                .split(ghost, Direction::Row, Placement::After, None)
                .map(|_| ()),
            unknown
        );
        assert_eq!(layout.close(ghost), unknown);
        assert_eq!(layout.focus(ghost), unknown);
        assert_eq!(layout.show(ghost, None), unknown);
        assert_eq!(layout.move_pane(ghost, a, Edge::Left), unknown);
        assert_eq!(layout.move_pane(a, ghost, Edge::Left), unknown);
        assert_eq!(layout.session_of(ghost).map(|_| ()), unknown);
        assert_eq!(layout, before);
    }

    #[test]
    fn the_number_of_panes_is_bounded() {
        let mut layout = Layout::default();
        let a = layout.focused();
        for _ in 1..MAX_PANES {
            layout
                .split(a, Direction::Row, Placement::After, None)
                .unwrap();
        }
        let before = layout.clone();

        assert_eq!(
            layout.split(a, Direction::Row, Placement::After, None),
            Err(LayoutError::TooManyPanes)
        );
        assert_eq!(layout, before);
        assert_eq!(layout.pane_count(), MAX_PANES);
    }

    #[test]
    fn the_layout_survives_a_round_trip_through_json() {
        let mut layout = Layout::default();
        let a = layout.focused();
        layout.show(a, Some(key("s-a"))).unwrap();
        let b = layout
            .split(a, Direction::Column, Placement::After, None)
            .unwrap();
        layout.focus(a).unwrap();

        let json = serde_json::to_value(&layout).unwrap();
        let read: Layout = serde_json::from_value(json.clone()).unwrap();

        assert_eq!(read, layout);
        assert_eq!(
            json,
            serde_json::json!({
                "root": {
                    "type": "split",
                    "id": 3,
                    "direction": "column",
                    "ratio": 0.5,
                    "first": { "type": "pane", "id": a.as_u32(), "session": "s-a" },
                    "second": { "type": "pane", "id": b.as_u32(), "session": null },
                },
                "focused": a.as_u32(),
                "nextId": 4,
            })
        );
    }

    #[test]
    fn sanitizing_repairs_what_a_file_may_contain() {
        let mut layout: Layout = serde_json::from_value(serde_json::json!({
            "root": {
                "type": "split", "id": 1, "direction": "row", "ratio": 42.0,
                "first": { "type": "pane", "id": 1, "session": "s-a" },
                "second": {
                    "type": "split", "id": 2, "direction": "column", "ratio": 0.25,
                    "first": { "type": "pane", "id": 2, "session": "s-a" },
                    "second": { "type": "pane", "id": 0, "session": "s-gone" },
                },
            },
            "focused": 77,
            "nextId": 1,
        }))
        .unwrap();

        let changed = layout.sanitize(|session| session.as_str() == "s-a");

        assert!(changed);
        layout.check_invariants().unwrap();
        assert_eq!(shown(&layout), [Some("s-a".to_owned()), None, None]);
        assert_eq!(layout.focused(), ids(&layout)[0]);
        let Node::Split { ratio, .. } = layout.root() else {
            panic!("the root is not a split");
        };
        assert!((ratio - MAX_RATIO).abs() < f64::EPSILON);

        // A valid layout is left alone.
        assert!(!layout.sanitize(|session| session.as_str() == "s-a"));
    }

    #[test]
    fn identifiers_near_the_end_of_their_range_are_renumbered() {
        let mut layout: Layout = serde_json::from_value(serde_json::json!({
            "root": {
                "type": "split", "id": 4_294_967_293_u32, "direction": "row", "ratio": 0.5,
                "first": { "type": "pane", "id": 4_294_967_294_u32, "session": "s-a" },
                "second": { "type": "pane", "id": 7, "session": "s-b" },
            },
            "focused": 7,
            "nextId": 4_294_967_295_u32,
        }))
        .unwrap();

        assert!(layout.sanitize(|_| true));

        layout.check_invariants().unwrap();
        assert_eq!(ids(&layout), [PaneId(2), PaneId(3)]);
        assert_eq!(layout.focused(), PaneId(3));
        assert_eq!(
            shown(&layout),
            [Some("s-a".to_owned()), Some("s-b".to_owned())]
        );
        // Splitting hands out identifiers no node has.
        for _ in 0..3 {
            let focused = layout.focused();
            layout
                .split(focused, Direction::Row, Placement::After, None)
                .unwrap();
            layout.check_invariants().unwrap();
        }
    }

    #[test]
    fn sanitizing_replaces_a_layout_with_too_many_panes() {
        let mut node = serde_json::json!({ "type": "pane", "id": 1 });
        for id in 0..=MAX_PANES {
            node = serde_json::json!({
                "type": "split", "id": 100 + id, "direction": "row", "ratio": 0.5,
                "first": node,
                "second": { "type": "pane", "id": 1000 + id },
            });
        }
        let mut layout: Layout = serde_json::from_value(serde_json::json!({
            "root": node, "focused": 1, "nextId": 5000,
        }))
        .unwrap();

        assert!(layout.sanitize(|_| true));

        assert_eq!(layout, Layout::default());
    }

    /// A small deterministic generator (xorshift), so that the random walk
    /// below is the same on every run and every platform.
    struct Random(u64);

    impl Random {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, bound: usize) -> usize {
            usize::try_from(self.next() % u64::try_from(bound).unwrap()).unwrap()
        }
    }

    /// Property test: whatever sequence of operations is applied, with valid
    /// and invalid arguments alike, the invariants hold, a refused operation
    /// changes nothing, and no session is lost except by the operations
    /// that are meant to hide one.
    #[test]
    fn random_operations_never_break_the_invariants() {
        let sessions: Vec<SessionKey> = (0..6).map(|n| key(&format!("s-{n}"))).collect();
        for seed in 1..=300u64 {
            let mut random = Random(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
            let mut layout = Layout::default();
            for step in 0..120 {
                let before = layout.clone();
                let panes = ids(&layout);
                // Sometimes a pane that does not exist.
                let pick_pane = |random: &mut Random| {
                    if random.below(12) == 0 {
                        PaneId(9_999)
                    } else {
                        panes[random.below(panes.len())]
                    }
                };
                let pane = pick_pane(&mut random);
                let other = pick_pane(&mut random);
                let session = match random.below(4) {
                    0 => None,
                    _ => Some(sessions[random.below(sessions.len())].clone()),
                };
                let direction = if random.below(2) == 0 {
                    Direction::Row
                } else {
                    Direction::Column
                };
                let placement = if random.below(2) == 0 {
                    Placement::Before
                } else {
                    Placement::After
                };
                let result: Result<(), LayoutError> = match random.below(7) {
                    0 | 1 => layout
                        .split(pane, direction, placement, session)
                        .map(|_| ()),
                    2 => layout.close(pane),
                    3 => {
                        let edge = [
                            Edge::Left,
                            Edge::Right,
                            Edge::Top,
                            Edge::Bottom,
                            Edge::Center,
                        ][random.below(5)];
                        layout.move_pane(pane, other, edge)
                    }
                    4 => {
                        let split = SplitId(u32::try_from(random.below(40)).unwrap());
                        let ratio = [0.5, -3.0, 0.0, 1.0, 0.77, f64::NAN][random.below(6)];
                        layout.resize(split, ratio).map(|_| ())
                    }
                    5 => layout.focus(pane),
                    _ => layout.show(pane, session),
                };

                if let Err(problem) = layout.check_invariants() {
                    panic!("seed {seed}, step {step}: {problem}\n{layout:#?}");
                }
                if result.is_err() {
                    assert_eq!(layout, before, "seed {seed}, step {step}: {result:?}");
                }
                // The tree survives a round trip through its file format.
                let json = serde_json::to_string(&layout).unwrap();
                let mut read: Layout = serde_json::from_str(&json).unwrap();
                assert!(!read.sanitize(|_| true), "seed {seed}, step {step}");
                assert_eq!(read, layout);
            }
        }
    }

    /// Moving and closing panes never loses a session that another pane
    /// shows, and moving never changes the set of panes.
    #[test]
    fn moving_panes_preserves_panes_and_sessions() {
        for seed in 1..=200u64 {
            let mut random = Random(seed.wrapping_mul(0xD6E8_FEB8_6659_FD93) | 1);
            let mut layout = Layout::default();
            let first = layout.focused();
            layout.show(first, Some(key("s-0"))).unwrap();
            for n in 1..8 {
                let panes = ids(&layout);
                let pane = panes[random.below(panes.len())];
                let direction = if random.below(2) == 0 {
                    Direction::Row
                } else {
                    Direction::Column
                };
                layout
                    .split(
                        pane,
                        direction,
                        Placement::After,
                        Some(key(&format!("s-{n}"))),
                    )
                    .unwrap();
            }

            for _ in 0..40 {
                let panes = ids(&layout);
                let pane = panes[random.below(panes.len())];
                let target = panes[random.below(panes.len())];
                let edge = [
                    Edge::Left,
                    Edge::Right,
                    Edge::Top,
                    Edge::Bottom,
                    Edge::Center,
                ][random.below(5)];
                let mut panes_before = ids(&layout);
                let mut sessions_before = shown(&layout);

                let moved = layout.move_pane(pane, target, edge);

                assert_eq!(moved.is_err(), pane == target);
                let mut panes_after = ids(&layout);
                let mut sessions_after = shown(&layout);
                panes_before.sort();
                panes_after.sort();
                sessions_before.sort();
                sessions_after.sort();
                assert_eq!(panes_after, panes_before, "seed {seed}");
                assert_eq!(sessions_after, sessions_before, "seed {seed}");
                layout.check_invariants().unwrap();
            }
        }
    }
}
