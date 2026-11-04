//! Session records written by the Claude Code hook and read by the overlay.
//!
//! One small JSON file per Claude Code session. The hook process only ever
//! replaces its own session's file (write to a temp file, then rename), so
//! readers never see a half-written record. The decision logic below is pure
//! and platform independent.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::paint::Look;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    #[default]
    Working,
    Attention,
    Failure,
    Done,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionRecord {
    pub session_id: String,
    pub phase: Phase,
    /// When the hook that produced this record started (ms since epoch).
    pub updated_ms: u64,
    /// When the current turn's prompt was submitted.
    pub turn_started_ms: u64,
    pub cwd: String,
    pub transcript_path: String,
    /// Top-level window the session runs in (VS Code, Claude, a terminal).
    pub host_hwnd: i64,
    pub host_pid: u32,
    pub host_exe: String,
    /// The Claude Code process itself; when it exits, the session is over.
    pub agent_pid: u32,
    pub agent_exe: String,
    pub last_event: String,
    pub message: String,
}

/// The subset of the hook's stdin JSON this feature reads.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct HookInput {
    pub hook_event_name: String,
    pub session_id: String,
    pub cwd: String,
    pub transcript_path: String,
    pub notification_type: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Update {
    /// Write this record. `refresh_host` asks the caller to work out which
    /// window the session runs in before writing.
    Write { record: SessionRecord, refresh_host: bool },
    Delete,
    Skip,
}

/// Map one hook event onto the session's stored record.
pub fn apply_event(existing: Option<&SessionRecord>, input: &HookInput, now_ms: u64) -> Update {
    if input.session_id.is_empty() {
        return Update::Skip;
    }
    // Async hooks can finish out of order: never let an older event overwrite
    // a newer one.
    if existing.is_some_and(|record| record.updated_ms > now_ms) {
        return Update::Skip;
    }
    let current = existing.map(|record| record.phase);
    let busy = matches!(current, None | Some(Phase::Working) | Some(Phase::Attention));

    let (phase, new_turn) = match input.hook_event_name.as_str() {
        "UserPromptSubmit" => (Phase::Working, true),
        // A tool finished, so whatever blocked the turn has been answered.
        "PostToolUse" | "PostToolUseFailure" | "PostToolBatch" | "PermissionDenied"
        | "ElicitationResult" | "SubagentStop" | "PreCompact" | "PostCompact" => {
            if !busy {
                return Update::Skip;
            }
            (Phase::Working, false)
        }
        "PermissionRequest" | "Elicitation" => (Phase::Attention, false),
        "Notification" => match notification_kind(input) {
            NotificationKind::NeedsUser => (Phase::Attention, false),
            NotificationKind::Resumed if busy => (Phase::Working, false),
            NotificationKind::Idle if current == Some(Phase::Working) => {
                // Waiting for input with no Stop seen: the turn was interrupted.
                return Update::Delete;
            }
            _ => return Update::Skip,
        },
        "Stop" => (Phase::Done, false),
        "StopFailure" => (Phase::Failure, false),
        "SessionEnd" => return Update::Delete,
        _ => return Update::Skip,
    };

    let mut record = existing.cloned().unwrap_or_default();
    record.session_id = input.session_id.clone();
    record.phase = phase;
    record.updated_ms = now_ms;
    if new_turn || record.turn_started_ms == 0 {
        record.turn_started_ms = now_ms;
    }
    if !input.cwd.is_empty() {
        record.cwd = input.cwd.clone();
    }
    if !input.transcript_path.is_empty() {
        record.transcript_path = input.transcript_path.clone();
    }
    record.last_event = input.hook_event_name.clone();
    record.message = input.message.chars().take(200).collect();
    let refresh_host = new_turn || record.host_hwnd == 0 || record.agent_pid == 0;
    Update::Write { record, refresh_host }
}

#[derive(Debug, PartialEq, Eq)]
enum NotificationKind {
    NeedsUser,
    Resumed,
    Idle,
    Other,
}

fn notification_kind(input: &HookInput) -> NotificationKind {
    match input.notification_type.as_str() {
        "permission_prompt" | "elicitation_dialog" | "elicitation_url_dialog" | "agent_needs_input" => {
            NotificationKind::NeedsUser
        }
        "elicitation_complete" | "elicitation_response" => NotificationKind::Resumed,
        "idle_prompt" => NotificationKind::Idle,
        "" => {
            // Older Claude Code builds send only the message text.
            let message = input.message.to_ascii_lowercase();
            if message.contains("permission") {
                NotificationKind::NeedsUser
            } else if message.contains("waiting for your input") {
                NotificationKind::Idle
            } else {
                NotificationKind::Other
            }
        }
        _ => NotificationKind::Other,
    }
}

/// Tunables for turning records into what the badge shows.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// Hide a working session after this long without any sign of life.
    pub stale_ms: u64,
    /// Stop reminding about a finished session after this long.
    pub done_ttl_ms: u64,
    /// Stop reminding about a blocked session after this long.
    pub attention_ttl_ms: u64,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            stale_ms: 10 * 60_000,
            done_ttl_ms: 30 * 60_000,
            attention_ttl_ms: 60 * 60_000,
        }
    }
}

/// Live facts the overlay gathers about one record.
#[derive(Clone, Copy, Debug, Default)]
pub struct Observation {
    /// The session's window is the active window.
    pub focused: bool,
    /// The user already saw this record (focused the window or clicked).
    pub acknowledged: bool,
    /// The transcript shows the turn was interrupted after the last hook.
    pub interrupted: bool,
    /// The user dismissed the working badge for this turn.
    pub muted: bool,
    /// The Claude Code process that wrote this record has exited.
    pub gone: bool,
    /// Last write to the transcript, a heartbeat for long tool calls.
    pub transcript_mtime_ms: u64,
}

/// What one session contributes to the badge, if anything.
pub fn session_look(record: &SessionRecord, obs: Observation, now_ms: u64, timing: Timing) -> Option<Look> {
    if obs.interrupted || obs.gone {
        return None;
    }
    let age = now_ms.saturating_sub(record.updated_ms);
    let alive = now_ms.saturating_sub(record.updated_ms.max(obs.transcript_mtime_ms));
    let look = match record.phase {
        Phase::Working => {
            if alive > timing.stale_ms || obs.muted {
                return None;
            }
            Look::Working
        }
        Phase::Attention => {
            if age > timing.attention_ttl_ms {
                return None;
            }
            if obs.acknowledged {
                // The user went to the window to answer; once they leave
                // again Claude is most likely carrying on.
                if alive > timing.stale_ms || obs.muted {
                    return None;
                }
                Look::Working
            } else {
                Look::Attention
            }
        }
        Phase::Failure => {
            if obs.acknowledged || age > timing.attention_ttl_ms {
                return None;
            }
            Look::Failure
        }
        Phase::Done => {
            if obs.acknowledged || age > timing.done_ttl_ms {
                return None;
            }
            Look::Done
        }
    };
    if obs.focused {
        return None;
    }
    Some(look)
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    pub look: Option<Look>,
    /// Session the badge is about; clicking it brings this one forward.
    pub primary: Option<String>,
    /// Sessions sharing the shown look.
    pub count: u32,
}

/// Combine every session into the one thing the badge shows. Blocked
/// sessions win, then finished ones, then working ones.
pub fn summarise<'a>(looks: impl IntoIterator<Item = (&'a SessionRecord, Look)>) -> Summary {
    let rank = |look: Look| match look {
        Look::Attention | Look::Failure => 3,
        Look::Done => 2,
        Look::Working => 1,
    };
    let mut best: Option<(u8, u64, &SessionRecord, Look)> = None;
    let mut counts: HashMap<u8, u32> = HashMap::new();
    for (record, look) in looks {
        let r = rank(look);
        *counts.entry(r).or_default() += 1;
        let better = match best {
            None => true,
            Some((br, bu, _, _)) => r > br || (r == br && record.updated_ms > bu),
        };
        if better {
            best = Some((r, record.updated_ms, record, look));
        }
    }
    match best {
        Some((r, _, record, look)) => Summary {
            look: Some(look),
            primary: Some(record.session_id.clone()),
            count: counts.get(&r).copied().unwrap_or(1),
        },
        None => Summary::default(),
    }
}

/// True when the last line of a transcript tail marks a user interrupt.
pub fn tail_shows_interrupt(tail: &str) -> bool {
    tail.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .is_some_and(|line| line.contains("[Request interrupted by user"))
}

/// File name for a session id that is safe on every file system.
pub fn file_name_for(session_id: &str) -> String {
    let safe: String = session_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .take(96)
        .collect();
    format!("{safe}.json")
}

pub fn read_record(path: &Path) -> Option<SessionRecord> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Replace a record atomically.
pub fn write_record(dir: &Path, record: &SessionRecord) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let target = dir.join(file_name_for(&record.session_id));
    let temp = dir.join(format!(
        ".{}.{}.tmp",
        file_name_for(&record.session_id),
        std::process::id()
    ));
    let json = serde_json::to_vec_pretty(record).map_err(std::io::Error::other)?;
    std::fs::write(&temp, json)?;
    std::fs::rename(&temp, &target).inspect_err(|_| {
        let _ = std::fs::remove_file(&temp);
    })
}

pub fn delete_record(dir: &Path, session_id: &str) {
    let _ = std::fs::remove_file(dir.join(file_name_for(session_id)));
}

/// Every record in the directory with its file modification time.
pub fn list_records(dir: &Path) -> Vec<(PathBuf, SessionRecord)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().is_some_and(|ext| ext == "json")
                && !path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with('.'))
        })
        .filter_map(|path| read_record(&path).map(|record| (path, record)))
        .collect()
}

/// Remove records nobody will look at again.
pub fn prune(dir: &Path, now_ms: u64, max_age_ms: u64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let old = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .map(|modified| now_ms.saturating_sub(system_time_ms(modified)) > max_age_ms)
            .unwrap_or(false);
        if old {
            let _ = std::fs::remove_file(path);
        }
    }
}

pub fn system_time_ms(time: std::time::SystemTime) -> u64 {
    time.duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn now_ms() -> u64 {
    system_time_ms(std::time::SystemTime::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(event: &str) -> HookInput {
        HookInput {
            hook_event_name: event.into(),
            session_id: "abc".into(),
            cwd: "C:\\proyectos\\sigi".into(),
            transcript_path: "C:\\t.jsonl".into(),
            ..Default::default()
        }
    }

    fn written(update: Update) -> SessionRecord {
        match update {
            Update::Write { record, .. } => record,
            other => panic!("expected a write, got {other:?}"),
        }
    }

    #[test]
    fn a_turn_goes_working_then_done() {
        let start = written(apply_event(None, &input("UserPromptSubmit"), 1_000));
        assert_eq!(start.phase, Phase::Working);
        assert_eq!(start.turn_started_ms, 1_000);
        let tool = written(apply_event(Some(&start), &input("PostToolUse"), 2_000));
        assert_eq!(tool.phase, Phase::Working);
        assert_eq!(tool.turn_started_ms, 1_000);
        let done = written(apply_event(Some(&tool), &input("Stop"), 3_000));
        assert_eq!(done.phase, Phase::Done);
    }

    #[test]
    fn permission_prompts_need_attention_until_a_tool_runs() {
        let start = written(apply_event(None, &input("UserPromptSubmit"), 1_000));
        let mut note = input("Notification");
        note.notification_type = "permission_prompt".into();
        let blocked = written(apply_event(Some(&start), &note, 2_000));
        assert_eq!(blocked.phase, Phase::Attention);
        let resumed = written(apply_event(Some(&blocked), &input("PostToolUse"), 3_000));
        assert_eq!(resumed.phase, Phase::Working);
    }

    #[test]
    fn late_events_never_overwrite_newer_ones() {
        let done = written(apply_event(None, &input("Stop"), 5_000));
        assert_eq!(apply_event(Some(&done), &input("PostToolUse"), 4_000), Update::Skip);
        // A heartbeat after the turn finished is ignored too.
        assert_eq!(apply_event(Some(&done), &input("PostToolUse"), 6_000), Update::Skip);
    }

    #[test]
    fn idle_without_stop_clears_the_session_and_session_end_deletes() {
        let start = written(apply_event(None, &input("UserPromptSubmit"), 1_000));
        let mut idle = input("Notification");
        idle.notification_type = "idle_prompt".into();
        assert_eq!(apply_event(Some(&start), &idle, 2_000), Update::Delete);
        let done = written(apply_event(Some(&start), &input("Stop"), 2_000));
        assert_eq!(apply_event(Some(&done), &idle, 3_000), Update::Skip);
        assert_eq!(apply_event(Some(&done), &input("SessionEnd"), 4_000), Update::Delete);
    }

    #[test]
    fn legacy_notifications_are_read_from_the_message() {
        let start = written(apply_event(None, &input("UserPromptSubmit"), 1_000));
        let mut note = input("Notification");
        note.message = "Claude needs your permission to use Bash".into();
        assert_eq!(written(apply_event(Some(&start), &note, 2_000)).phase, Phase::Attention);
    }

    #[test]
    fn focus_and_acknowledgement_hide_the_badge() {
        let timing = Timing::default();
        let mut record = written(apply_event(None, &input("Stop"), 10_000));
        let shown = session_look(&record, Observation::default(), 11_000, timing);
        assert_eq!(shown, Some(Look::Done));
        let focused = Observation { focused: true, ..Default::default() };
        assert_eq!(session_look(&record, focused, 11_000, timing), None);
        let acked = Observation { acknowledged: true, ..Default::default() };
        assert_eq!(session_look(&record, acked, 11_000, timing), None);
        record.phase = Phase::Attention;
        assert_eq!(session_look(&record, acked, 11_000, timing), Some(Look::Working));
    }

    #[test]
    fn stale_and_interrupted_work_is_hidden_but_transcript_keeps_it_alive() {
        let timing = Timing::default();
        let record = written(apply_event(None, &input("UserPromptSubmit"), 0));
        let late = timing.stale_ms + 1;
        assert_eq!(session_look(&record, Observation::default(), late, timing), None);
        let heartbeat = Observation { transcript_mtime_ms: late - 5, ..Default::default() };
        assert_eq!(session_look(&record, heartbeat, late, timing), Some(Look::Working));
        let interrupted = Observation { interrupted: true, ..Default::default() };
        assert_eq!(session_look(&record, interrupted, 10, timing), None);
        let gone = Observation { gone: true, ..Default::default() };
        assert_eq!(session_look(&record, gone, 10, timing), None);
        let muted = Observation { muted: true, ..Default::default() };
        assert_eq!(session_look(&record, muted, 10, timing), None);
    }

    #[test]
    fn summary_prefers_blocked_then_finished_then_working() {
        let mut a = written(apply_event(None, &input("UserPromptSubmit"), 1));
        a.session_id = "a".into();
        let mut b = a.clone();
        b.session_id = "b".into();
        b.updated_ms = 5;
        let mut c = a.clone();
        c.session_id = "c".into();
        let summary = summarise([(&a, Look::Working), (&b, Look::Working)]);
        assert_eq!(summary.look, Some(Look::Working));
        assert_eq!(summary.primary.as_deref(), Some("b"));
        assert_eq!(summary.count, 2);
        let summary = summarise([(&a, Look::Working), (&b, Look::Done), (&c, Look::Attention)]);
        assert_eq!(summary.look, Some(Look::Attention));
        assert_eq!(summary.primary.as_deref(), Some("c"));
        assert_eq!(summarise(std::iter::empty()), Summary::default());
    }

    #[test]
    fn interrupt_marker_is_read_from_the_last_line() {
        let tail = "{\"type\":\"assistant\"}\n{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"[Request interrupted by user]\"}]}}\n";
        assert!(tail_shows_interrupt(tail));
        assert!(!tail_shows_interrupt("{\"type\":\"user\"}\n{\"type\":\"assistant\"}\n"));
    }

    #[test]
    fn records_round_trip_through_files() {
        let dir = std::env::temp_dir().join(format!("ccum-activity-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let record = written(apply_event(None, &input("UserPromptSubmit"), 42));
        write_record(&dir, &record).unwrap();
        let listed = list_records(&dir);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].1, record);
        delete_record(&dir, "abc");
        assert!(list_records(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(file_name_for("a/b:c"), "a_b_c.json");
    }
}
