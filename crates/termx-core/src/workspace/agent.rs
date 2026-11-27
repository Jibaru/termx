//! What the agent of a session is doing, from the hooks of Claude Code.
//!
//! The hooks of the termx plugin report every event of Claude Code to
//! termx ([`termx_protocol::HookEvent`]); a [`Tracker`] per session turns
//! them into the [`AgentState`] the sidebar shows. termx never reads the
//! transcript of Claude Code, and never guesses from what is printed: a
//! state comes from a hook, or degrades to [`AgentStatus::Unknown`] when
//! termx can no longer vouch for it.
//!
//! # Events
//!
//! | Event                                        | Becomes                                     |
//! | -------------------------------------------- | ------------------------------------------- |
//! | `SessionStart` (`startup`, `resume`, `clear`, `fork`) | `unknown` (Claude waits at its prompt; nothing asked yet) |
//! | `SessionStart` (`compact`)                   | unchanged                                   |
//! | `UserPromptSubmit`                           | `working`                                   |
//! | `PreToolUse`                                 | `working`; `waitingForInput` for `AskUserQuestion` and `ExitPlanMode` |
//! | `PostToolUse`, `PostToolUseFailure`, `PermissionDenied` | `working`                        |
//! | `PermissionRequest`                          | `waitingForPermission`                      |
//! | `Notification` `permission_prompt`           | `waitingForPermission`                      |
//! | `Notification` `elicitation_dialog`, `elicitation_url_dialog`, `agent_needs_input` | `waitingForInput` |
//! | `Notification` `elicitation_complete`, `elicitation_response` | `working`                  |
//! | `Notification` `idle_prompt`                 | `waitingForInput` unless already waiting or `done` |
//! | `Elicitation` / `ElicitationResult`          | `waitingForInput` / `working`               |
//! | `Stop`                                       | `done`                                      |
//! | `StopFailure`                                | `waitingForInput`                           |
//! | `SubagentStart`, `SubagentStop`, others      | unchanged (they still show Claude is alive) |
//! | `SessionEnd`                                 | no agent (`null`)                           |
//!
//! `done` and `waitingForInput` are both Claude at its prompt, and differ
//! in what the user is to do. `done` is a turn that ended normally
//! (`Stop`): Claude finished what it was asked, and the user reads the
//! result when they like. `waitingForInput` is Claude needing an answer to
//! go on, or a turn that could not end (`StopFailure`: a rate limit, an
//! authentication or server error, which the user must deal with). Claude's
//! own reminder that it waits (`idle_prompt`, about 60 s after a turn) does
//! not turn `done` into `waitingForInput`: nothing new is asked.
//!
//! # Waiting
//!
//! `waitingForPermission`, `waitingForInput` and `done` all wait for the
//! user. [`AgentState::waiting_since`] is set when the agent starts waiting
//! and kept while it goes from one of them to another (it waited all
//! along); it is cleared when the agent works again. "The session that has
//! waited longest" is the one with the smallest `waiting_since`.
//!
//! # Order, repeats and other agents
//!
//! Each hook runs in a process of its own, so reports can arrive out of
//! order or twice. Each carries the time its hook started; a report older
//! than the last one applied is dropped, and applying the same one twice
//! changes nothing.
//!
//! One terminal can run more than one Claude Code: a `claude -p` started by
//! the agent itself, or a new `claude` in a shell. The tracker follows one
//! conversation, the *current* one, and ignores the others. Another
//! conversation becomes the current one when it starts (`SessionStart`)
//! and the current one has ended, or was replaced on purpose (`/clear`,
//! `/resume`, `--fork-session`: sources `clear`, `resume`, `fork`,
//! `compact`), or does not work at the moment (a `claude -p` started by the
//! agent runs while the agent works, so it never takes over). Its
//! identifier becomes the conversation of the session, which `claude
//! --resume` resumes.
//!
//! Hooks also run for subagents. A subagent that asks for something makes
//! the session wait like the main agent does; only that subagent, or the
//! main agent, ends that wait. A subagent never turns `done` into
//! `working`: a turn that ended stays ended while background work goes on.
//!
//! # Staleness
//!
//! A state is only as good as the last report. termx degrades it, and
//! marks it [`stale`](AgentState::stale), rather than show something it
//! cannot vouch for:
//!
//! - **Working in silence**: `working` with neither a report nor any output
//!   in the terminal for [`WORKING_SILENCE_LIMIT`] becomes `unknown`.
//!   Claude Code animates its status while it works, so a working agent is
//!   never silent that long; one that is silent has stopped without
//!   saying so.
//! - **Process gone**: the hooks say which process of Claude Code ran them
//!   ([`termx_protocol::HookEvent::pid`]). Once two reports agree on it,
//!   termx checks every [`AGENT_CHECK_INTERVAL`] that it still runs; when
//!   it does not, whatever the state was, it becomes `unknown`.
//! - **Terminal gone**: when the process of the terminal exits, or the
//!   session stops or restarts, there is no agent any more (`null`).
//!
//! The agent state is never saved: a restored session has none until its
//! Claude Code reports again. Only the identifier of the conversation is
//! saved, to resume it.

use std::time::Duration;

use termx_protocol::HookEvent;

use super::model::{AgentState, AgentStatus};

/// How long an agent may be `working` without a report from its hooks and
/// without printing anything before termx stops vouching for it. See the
/// [module documentation](self#staleness).
pub const WORKING_SILENCE_LIMIT: Duration = Duration::from_secs(60);

/// How often termx checks that the process of Claude Code still runs, and
/// that a working agent is not silent. See the
/// [module documentation](self#staleness).
pub const AGENT_CHECK_INTERVAL: Duration = Duration::from_secs(2);

/// How far in the future a report may be dated, in microseconds. A report
/// dated later is taken as dated now: the clock of a hook is the clock of
/// this machine, so that only happens when the clock moved.
const MAX_FUTURE_US: u64 = 60 * 1_000_000;

/// The sources of `SessionStart` that replace the conversation on purpose.
const REPLACING_SOURCES: &[&str] = &["clear", "resume", "fork", "compact"];

/// What a report changed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Applied {
    /// The agent state changed.
    pub changed: bool,
    /// The conversation the session follows now, when it changed.
    pub conversation: Option<String>,
}

/// Follows the agent of one session. See the [module documentation](self).
#[derive(Debug, Default, Clone)]
pub(crate) struct Tracker {
    state: Option<AgentState>,
    /// The conversation followed.
    current: Option<String>,
    /// The current conversation said it ended (`SessionEnd`), or its
    /// process is gone.
    ended: bool,
    /// When the last report applied was made, in microseconds.
    last_at: u64,
    /// When the last report from the current conversation arrived, in
    /// milliseconds.
    last_report_ms: u64,
    /// The subagent the agent waits for, when a subagent asked.
    waiting_for: Option<String>,
    /// The process of Claude Code once two reports agreed on it.
    process: Option<u32>,
    /// The process the last report named, not confirmed yet.
    candidate: Option<u32>,
}

impl Tracker {
    /// The state to show.
    pub fn state(&self) -> Option<&AgentState> {
        self.state.as_ref()
    }

    /// Forgets everything: the terminal of the session started again or
    /// went away.
    pub fn reset(&mut self) -> bool {
        let had = self.state.is_some();
        *self = Self::default();
        had
    }

    /// Applies a report that arrived at `now_ms`.
    pub fn apply(&mut self, event: &HookEvent, now_ms: u64) -> Applied {
        let at = event
            .at
            .min(now_ms.saturating_mul(1000).saturating_add(MAX_FUTURE_US));
        if at < self.last_at {
            return Applied::default();
        }
        let name = event.name.as_str();
        let id = event.claude_session_id.as_deref();
        let mut applied = Applied::default();

        // Which conversation this is.
        let is_current = match (&self.current, id) {
            (_, None) => true,
            (None, Some(id)) => {
                self.follow(id);
                applied.conversation = Some(id.to_owned());
                true
            }
            (Some(current), Some(id)) if current == id => true,
            (Some(_), Some(id)) => {
                let takes_over = self.ended
                    || (name == "SessionStart"
                        && (event
                            .source
                            .as_deref()
                            .is_some_and(|source| REPLACING_SOURCES.contains(&source))
                            || !self.is_working()));
                if takes_over {
                    self.follow(id);
                    applied.conversation = Some(id.to_owned());
                }
                takes_over
            }
        };
        if !is_current {
            return applied;
        }
        self.last_at = at;
        self.last_report_ms = now_ms;
        if event.agent.is_none() {
            self.note_process(event.pid);
        }

        let before = self.state.clone();
        if name == "SessionEnd" {
            self.state = None;
            self.ended = true;
            self.process = None;
            self.candidate = None;
            self.waiting_for = None;
        } else {
            self.ended = false;
            if let Some(status) = self.next_status(event) {
                self.enter(status, event.agent.as_deref(), now_ms);
            } else if self.state.is_none() || self.state.as_ref().is_some_and(|s| s.stale) {
                // Any report shows the agent is there again.
                self.enter(self.status().unwrap_or(AgentStatus::Unknown), None, now_ms);
                if let Some(state) = &mut self.state {
                    state.stale = false;
                }
            }
        }
        applied.changed = self.state != before;
        applied
    }

    /// Degrades a state termx can no longer vouch for. `last_output_ms` is
    /// when the terminal last printed something, `silence_limit` is
    /// [`WORKING_SILENCE_LIMIT`] but for tests, and `running` says whether
    /// a process runs. Returns whether the state changed.
    pub fn check(
        &mut self,
        now_ms: u64,
        last_output_ms: u64,
        silence_limit: Duration,
        running: impl Fn(u32) -> bool,
    ) -> bool {
        let Some(state) = &self.state else {
            return false;
        };
        if let Some(process) = self.process
            && !running(process)
        {
            self.process = None;
            self.candidate = None;
            self.ended = true;
            self.degrade(now_ms);
            return true;
        }
        let silent_since = self.last_report_ms.max(last_output_ms);
        let limit = u64::try_from(silence_limit.as_millis()).unwrap_or(u64::MAX);
        if state.status == AgentStatus::Working && now_ms.saturating_sub(silent_since) >= limit {
            self.degrade(now_ms);
            return true;
        }
        false
    }

    /// Whether [`check`](Self::check) has anything to watch.
    pub fn needs_checks(&self) -> bool {
        self.state.as_ref().is_some_and(|state| {
            self.process.is_some() || state.status == AgentStatus::Working && !state.stale
        })
    }

    fn status(&self) -> Option<AgentStatus> {
        self.state.as_ref().map(|state| state.status)
    }

    fn is_working(&self) -> bool {
        self.state
            .as_ref()
            .is_some_and(|state| state.status == AgentStatus::Working && !state.stale)
    }

    /// Starts following the conversation `id`.
    fn follow(&mut self, id: &str) {
        self.current = Some(id.to_owned());
        self.ended = false;
        self.process = None;
        self.candidate = None;
        self.waiting_for = None;
    }

    /// Confirms the process of Claude Code once two reports name it.
    fn note_process(&mut self, pid: Option<u32>) {
        let Some(pid) = pid else { return };
        if self.process.is_some() {
            return;
        }
        if self.candidate == Some(pid) {
            self.process = Some(pid);
        } else {
            self.candidate = Some(pid);
        }
    }

    /// The status `event` leads to, or `None` when it changes nothing.
    fn next_status(&self, event: &HookEvent) -> Option<AgentStatus> {
        use AgentStatus::{Done, Unknown, WaitingForInput, WaitingForPermission, Working};

        let current = self.status();
        let next = match event.name.as_str() {
            "SessionStart" => {
                if event.source.as_deref() == Some("compact") {
                    return current.is_none().then_some(Unknown);
                }
                Unknown
            }
            "UserPromptSubmit" => Working,
            "PreToolUse" => match event.tool.as_deref() {
                Some("AskUserQuestion" | "ExitPlanMode") => WaitingForInput,
                _ => Working,
            },
            "PostToolUse" | "PostToolUseFailure" | "PermissionDenied" | "ElicitationResult" => {
                Working
            }
            "PermissionRequest" => WaitingForPermission,
            "Elicitation" => WaitingForInput,
            "Notification" => match event.notification_type.as_deref()? {
                "permission_prompt" => WaitingForPermission,
                "elicitation_dialog" | "elicitation_url_dialog" | "agent_needs_input" => {
                    WaitingForInput
                }
                "elicitation_complete" | "elicitation_response" => Working,
                "idle_prompt" => match current {
                    Some(WaitingForPermission | WaitingForInput | Done) => return None,
                    _ => WaitingForInput,
                },
                _ => return None,
            },
            "Stop" => Done,
            "StopFailure" => WaitingForInput,
            "SubagentStop" if event.agent.is_some() && event.agent == self.waiting_for => Working,
            _ => return None,
        };
        // What a subagent may not do: end a wait it did not cause, or
        // reopen a turn that ended.
        if let Some(agent) = &event.agent {
            let waiting = matches!(current, Some(WaitingForPermission | WaitingForInput));
            if next == Working {
                if current == Some(Done) {
                    return None;
                }
                if waiting && self.waiting_for.as_ref() != Some(agent) {
                    return None;
                }
            }
        }
        Some(next)
    }

    /// Enters `status`, asked by the subagent `agent` if any.
    fn enter(&mut self, status: AgentStatus, agent: Option<&str>, now_ms: u64) {
        let waits = is_waiting(status);
        if waits && !self.status().is_some_and(is_waiting) {
            self.waiting_for = agent.map(str::to_owned);
        } else if !waits {
            self.waiting_for = None;
        }
        let previous = self.state.take();
        let waiting_since = match (&previous, waits) {
            (_, false) => None,
            (Some(previous), true) if !previous.stale => previous.waiting_since.or(Some(now_ms)),
            _ => Some(now_ms),
        };
        let since = match &previous {
            Some(previous) if previous.status == status && !previous.stale => previous.since,
            _ => now_ms,
        };
        self.state = Some(AgentState {
            status,
            waiting_since,
            since,
            stale: false,
        });
    }

    /// The agent may not be doing what was last reported.
    fn degrade(&mut self, now_ms: u64) {
        self.waiting_for = None;
        self.state = Some(AgentState {
            status: AgentStatus::Unknown,
            waiting_since: None,
            since: now_ms,
            stale: true,
        });
    }
}

/// Whether the agent waits for the user in `status`.
fn is_waiting(status: AgentStatus) -> bool {
    matches!(
        status,
        AgentStatus::WaitingForPermission | AgentStatus::WaitingForInput | AgentStatus::Done
    )
}

/// Whether the process `pid` still runs.
#[cfg(unix)]
pub(crate) fn process_is_running(pid: u32) -> bool {
    let Some(pid) = i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    else {
        return false;
    };
    match rustix::process::test_kill_process(pid) {
        Ok(()) => true,
        Err(error) => error == rustix::io::Errno::PERM,
    }
}

/// Whether the process `pid` still runs.
#[cfg(windows)]
#[allow(unsafe_code)]
pub(crate) fn process_is_running(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    // SAFETY: plain calls; the handle is closed before returning.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut code = 0u32;
        let read = GetExitCodeProcess(handle, &raw mut code);
        CloseHandle(handle);
        read != 0 && code == STILL_ACTIVE.cast_unsigned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use AgentStatus::{Done, Unknown, WaitingForInput, WaitingForPermission, Working};

    const ID: &str = "0b9d6f7e-3c1a-4f3e-9a55-2f6f1f0f8a10";

    /// A report of the main agent of `ID`, made at `at` milliseconds.
    fn event(name: &str, at_ms: u64) -> HookEvent {
        HookEvent {
            name: name.to_owned(),
            claude_session_id: Some(ID.to_owned()),
            notification_type: None,
            tool: None,
            source: None,
            reason: None,
            agent: None,
            pid: None,
            at: at_ms * 1000,
        }
    }

    fn notification(kind: &str, at_ms: u64) -> HookEvent {
        HookEvent {
            notification_type: Some(kind.to_owned()),
            ..event("Notification", at_ms)
        }
    }

    fn tool(name: &str, tool: &str, at_ms: u64) -> HookEvent {
        HookEvent {
            tool: Some(tool.to_owned()),
            ..event(name, at_ms)
        }
    }

    fn start(source: &str, id: &str, at_ms: u64) -> HookEvent {
        HookEvent {
            source: Some(source.to_owned()),
            claude_session_id: Some(id.to_owned()),
            ..event("SessionStart", at_ms)
        }
    }

    fn of_subagent(event: HookEvent, agent: &str) -> HookEvent {
        HookEvent {
            agent: Some(agent.to_owned()),
            ..event
        }
    }

    /// Applies each event at the time it was made.
    fn run(tracker: &mut Tracker, events: &[HookEvent]) {
        for event in events {
            tracker.apply(event, event.at / 1000);
        }
    }

    fn status(tracker: &Tracker) -> Option<AgentStatus> {
        tracker.state().map(|state| state.status)
    }

    #[test]
    fn every_event_maps_to_its_status() {
        let cases: Vec<(HookEvent, Option<AgentStatus>)> = vec![
            (start("startup", ID, 1), Some(Unknown)),
            (event("UserPromptSubmit", 1), Some(Working)),
            (event("PreToolUse", 1), Some(Working)),
            (
                tool("PreToolUse", "AskUserQuestion", 1),
                Some(WaitingForInput),
            ),
            (tool("PreToolUse", "ExitPlanMode", 1), Some(WaitingForInput)),
            (event("PostToolUse", 1), Some(Working)),
            (event("PostToolUseFailure", 1), Some(Working)),
            (event("PermissionDenied", 1), Some(Working)),
            (event("PermissionRequest", 1), Some(WaitingForPermission)),
            (
                notification("permission_prompt", 1),
                Some(WaitingForPermission),
            ),
            (notification("elicitation_dialog", 1), Some(WaitingForInput)),
            (
                notification("elicitation_url_dialog", 1),
                Some(WaitingForInput),
            ),
            (notification("agent_needs_input", 1), Some(WaitingForInput)),
            (notification("elicitation_complete", 1), Some(Working)),
            (notification("idle_prompt", 1), Some(WaitingForInput)),
            (event("Elicitation", 1), Some(WaitingForInput)),
            (event("ElicitationResult", 1), Some(Working)),
            (event("Stop", 1), Some(Done)),
            (event("StopFailure", 1), Some(WaitingForInput)),
            // Not about the state: an agent appears, nothing more.
            (event("SubagentStart", 1), Some(Unknown)),
            (event("SubagentStop", 1), Some(Unknown)),
            (notification("auth_success", 1), Some(Unknown)),
            (event("SomethingNew", 1), Some(Unknown)),
            (event("SessionEnd", 1), None),
        ];
        for (event, expected) in cases {
            let mut tracker = Tracker::default();
            tracker.apply(&event, 1);
            assert_eq!(
                status(&tracker),
                expected,
                "{} {:?}",
                event.name,
                event.notification_type
            );
        }
    }

    #[test]
    fn a_turn_with_a_permission_prompt() {
        let mut tracker = Tracker::default();
        let mut seen = Vec::new();
        for event in [
            start("startup", ID, 1),
            event("UserPromptSubmit", 2),
            event("PreToolUse", 3),
            event("PermissionRequest", 4),
            notification("permission_prompt", 10),
            event("PostToolUse", 20),
            tool("PreToolUse", "AskUserQuestion", 30),
            event("PostToolUse", 40),
            event("Stop", 50),
            notification("idle_prompt", 110),
        ] {
            tracker.apply(&event, event.at / 1000);
            seen.push((
                status(&tracker).unwrap(),
                tracker.state().unwrap().waiting_since,
            ));
        }
        assert_eq!(
            seen,
            [
                (Unknown, None),
                (Working, None),
                (Working, None),
                (WaitingForPermission, Some(4)),
                // The late notification of the same prompt: waiting since the request.
                (WaitingForPermission, Some(4)),
                (Working, None),
                (WaitingForInput, Some(30)),
                (Working, None),
                (Done, Some(50)),
                // Claude's reminder does not make a finished turn ask anything.
                (Done, Some(50)),
            ]
        );
    }

    #[test]
    fn waiting_since_is_kept_from_one_wait_to_the_next() {
        let mut tracker = Tracker::default();
        run(
            &mut tracker,
            &[
                event("UserPromptSubmit", 1),
                event("PermissionRequest", 5),
                event("StopFailure", 9),
            ],
        );
        let state = tracker.state().unwrap();
        assert_eq!(state.status, WaitingForInput);
        assert_eq!(state.waiting_since, Some(5));
        assert_eq!(state.since, 9);
    }

    #[test]
    fn since_is_when_the_status_was_entered() {
        let mut tracker = Tracker::default();
        run(
            &mut tracker,
            &[
                event("UserPromptSubmit", 1),
                event("PreToolUse", 5),
                event("PostToolUse", 9),
            ],
        );
        assert_eq!(tracker.state().unwrap().since, 1);
    }

    #[test]
    fn reports_that_arrive_late_or_twice_change_nothing() {
        let mut tracker = Tracker::default();
        run(
            &mut tracker,
            &[event("UserPromptSubmit", 1), event("Stop", 10)],
        );
        // A PreToolUse made before the Stop arrives after it.
        let late = tracker.apply(&event("PreToolUse", 5), 11);
        assert!(!late.changed);
        assert_eq!(status(&tracker), Some(Done));

        // The same report twice.
        let mut tracker = Tracker::default();
        run(
            &mut tracker,
            &[event("UserPromptSubmit", 1), event("PermissionRequest", 2)],
        );
        let before = tracker.state().cloned();
        let again = tracker.apply(&event("PermissionRequest", 2), 3);
        assert!(!again.changed);
        assert_eq!(tracker.state().cloned(), before);
    }

    #[test]
    fn a_report_dated_in_the_future_does_not_shut_out_the_next_ones() {
        let mut tracker = Tracker::default();
        // Dated a day ahead, received at 1 s.
        tracker.apply(&event("UserPromptSubmit", 86_400_000), 1_000);
        tracker.apply(&event("Stop", 70_000), 70_000);
        assert_eq!(status(&tracker), Some(Done));
    }

    #[test]
    fn the_conversation_is_captured_and_follows_clear_and_resume() {
        let mut tracker = Tracker::default();
        let first = tracker.apply(&start("startup", ID, 1), 1);
        assert_eq!(first.conversation.as_deref(), Some(ID));
        run(
            &mut tracker,
            &[event("UserPromptSubmit", 2), event("Stop", 3)],
        );

        // `/clear`: the old conversation ends, a new one starts.
        let end = tracker.apply(&event("SessionEnd", 4), 4);
        assert!(end.changed);
        assert_eq!(status(&tracker), None);
        let cleared = tracker.apply(&start("clear", "new-id", 5), 5);
        assert_eq!(cleared.conversation.as_deref(), Some("new-id"));
        assert_eq!(status(&tracker), Some(Unknown));

        // `/resume` of another one, without a SessionEnd first.
        tracker.apply(
            &HookEvent {
                claude_session_id: Some("new-id".to_owned()),
                ..event("UserPromptSubmit", 6)
            },
            6,
        );
        let resumed = tracker.apply(&start("resume", "older-id", 7), 7);
        assert_eq!(resumed.conversation.as_deref(), Some("older-id"));
    }

    #[test]
    fn a_claude_started_by_the_agent_does_not_take_over() {
        let mut tracker = Tracker::default();
        run(
            &mut tracker,
            &[
                start("startup", ID, 1),
                event("UserPromptSubmit", 2),
                event("PreToolUse", 3),
            ],
        );

        // `claude -p` run by a tool of the agent, in the same terminal.
        let nested = [
            start("startup", "nested", 4),
            HookEvent {
                claude_session_id: Some("nested".to_owned()),
                ..event("UserPromptSubmit", 5)
            },
            HookEvent {
                claude_session_id: Some("nested".to_owned()),
                ..event("Stop", 6)
            },
            HookEvent {
                claude_session_id: Some("nested".to_owned()),
                ..event("SessionEnd", 7)
            },
        ];
        for event in &nested {
            let applied = tracker.apply(event, event.at / 1000);
            assert_eq!(applied, Applied::default(), "{}", event.name);
        }
        assert_eq!(status(&tracker), Some(Working));
    }

    #[test]
    fn a_new_claude_takes_over_once_the_last_one_is_idle_or_gone() {
        let mut tracker = Tracker::default();
        run(&mut tracker, &[start("startup", ID, 1), event("Stop", 2)]);
        // The user quit Claude Code without a SessionEnd reaching termx,
        // then started it again in the same shell.
        let again = tracker.apply(&start("startup", "second", 3), 3);
        assert_eq!(again.conversation.as_deref(), Some("second"));

        let mut tracker = Tracker::default();
        run(
            &mut tracker,
            &[start("startup", ID, 1), event("SessionEnd", 2)],
        );
        let next = tracker.apply(
            &HookEvent {
                claude_session_id: Some("third".to_owned()),
                ..event("UserPromptSubmit", 3)
            },
            3,
        );
        assert_eq!(next.conversation.as_deref(), Some("third"));
        assert_eq!(status(&tracker), Some(Working));
    }

    #[test]
    fn a_subagent_ends_only_its_own_wait_and_never_reopens_a_turn() {
        let mut tracker = Tracker::default();
        run(
            &mut tracker,
            &[
                event("UserPromptSubmit", 1),
                of_subagent(event("PermissionRequest", 2), "a"),
                // Another subagent carries on meanwhile.
                of_subagent(event("PostToolUse", 3), "b"),
            ],
        );
        assert_eq!(status(&tracker), Some(WaitingForPermission));
        tracker.apply(&of_subagent(event("PostToolUse", 4), "a"), 4);
        assert_eq!(status(&tracker), Some(Working));

        // A subagent that asked and then stopped.
        run(
            &mut tracker,
            &[
                of_subagent(event("PermissionRequest", 5), "c"),
                of_subagent(event("SubagentStop", 6), "c"),
            ],
        );
        assert_eq!(status(&tracker), Some(Working));

        // Background work after the turn ended.
        run(
            &mut tracker,
            &[event("Stop", 7), of_subagent(event("PreToolUse", 8), "d")],
        );
        assert_eq!(status(&tracker), Some(Done));
        // The main agent can.
        tracker.apply(&event("UserPromptSubmit", 9), 9);
        assert_eq!(status(&tracker), Some(Working));
    }

    #[test]
    fn a_silent_working_agent_goes_stale() {
        let limit = u64::try_from(WORKING_SILENCE_LIMIT.as_millis()).unwrap();
        let mut tracker = Tracker::default();
        tracker.apply(&event("UserPromptSubmit", 1_000), 1_000);

        // Output keeps it fresh.
        assert!(!tracker.check(
            1_000 + limit + 10,
            1_000 + limit,
            WORKING_SILENCE_LIMIT,
            |_| true
        ));
        assert_eq!(status(&tracker), Some(Working));
        // Silence does not.
        assert!(tracker.check(
            1_000 + 2 * limit,
            1_000 + limit - 1,
            WORKING_SILENCE_LIMIT,
            |_| true
        ));
        let state = tracker.state().unwrap();
        assert_eq!((state.status, state.stale), (Unknown, true));
        assert!(!tracker.needs_checks());

        // The next report brings it back.
        tracker.apply(
            &event("PreToolUse", 1_000 + 2 * limit + 5),
            1_000 + 2 * limit + 5,
        );
        let state = tracker.state().unwrap();
        assert_eq!((state.status, state.stale), (Working, false));
    }

    #[test]
    fn a_waiting_agent_does_not_go_stale_from_silence() {
        let mut tracker = Tracker::default();
        run(
            &mut tracker,
            &[event("UserPromptSubmit", 1), event("Stop", 2)],
        );
        assert!(!tracker.check(10_000_000, 0, WORKING_SILENCE_LIMIT, |_| true));
        assert_eq!(status(&tracker), Some(Done));
    }

    #[test]
    fn an_agent_whose_process_is_gone_goes_stale_whatever_it_did() {
        let with_pid = |name: &str, at: u64| HookEvent {
            pid: Some(4242),
            ..event(name, at)
        };
        let mut tracker = Tracker::default();
        run(
            &mut tracker,
            &[
                with_pid("UserPromptSubmit", 1),
                with_pid("PermissionRequest", 2),
            ],
        );
        assert!(tracker.needs_checks());
        assert!(!tracker.check(3, 3, WORKING_SILENCE_LIMIT, |pid| pid == 4242));

        assert!(tracker.check(4, 4, WORKING_SILENCE_LIMIT, |_| false));

        let state = tracker.state().unwrap();
        assert_eq!(
            (state.status, state.stale, state.waiting_since),
            (Unknown, true, None)
        );
        // And a new Claude in the same terminal is followed at once.
        let next = tracker.apply(&start("startup", "after", 5), 5);
        assert_eq!(next.conversation.as_deref(), Some("after"));
    }

    #[test]
    fn a_process_named_by_one_report_only_is_not_trusted() {
        // Each hook names a different process: a wrapper between Claude
        // Code and the hook, which ends with the hook.
        let mut tracker = Tracker::default();
        for (n, at) in [(100, 1), (101, 2), (102, 3)] {
            tracker.apply(
                &HookEvent {
                    pid: Some(n),
                    ..event("PermissionRequest", at)
                },
                at,
            );
        }
        assert!(!tracker.check(4, 4, WORKING_SILENCE_LIMIT, |_| false));
        assert_eq!(status(&tracker), Some(WaitingForPermission));
    }

    #[test]
    fn a_reset_forgets_everything() {
        let mut tracker = Tracker::default();
        run(&mut tracker, &[start("startup", ID, 1), event("Stop", 2)]);
        assert!(tracker.reset());
        assert_eq!(tracker.state(), None);
        // The next report, even an older one, is welcome.
        let applied = tracker.apply(&start("resume", ID, 1), 1);
        assert_eq!(applied.conversation.as_deref(), Some(ID));
    }

    #[test]
    fn this_process_is_running_and_an_impossible_one_is_not() {
        assert!(process_is_running(std::process::id()));
        assert!(!process_is_running(u32::MAX - 7));
    }
}
