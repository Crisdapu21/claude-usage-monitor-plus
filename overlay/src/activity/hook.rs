//! `--activity-hook`: the short-lived process Claude Code runs on each hook
//! event. It reads the event from stdin, updates the session record and
//! exits. It never prints anything (UserPromptSubmit stdout would reach
//! Claude's context) and always exits 0 so it can never block a turn.

use std::io::Read;
use std::sync::mpsc;
use std::time::Duration;

use super::process;
use super::settings;
use super::store::{self, HookInput, SessionRecord, Update};

pub fn run() -> i32 {
    let started = store::now_ms();
    let Some(text) = read_stdin(Duration::from_secs(3)) else {
        return 0;
    };
    let Ok(input) = serde_json::from_str::<HookInput>(&text) else {
        return 0;
    };
    let dir = settings::sessions_dir();
    let path = dir.join(store::file_name_for(&input.session_id));
    let existing = store::read_record(&path);
    match store::apply_event(existing.as_ref(), &input, started) {
        Update::Write { mut record, refresh_host } => {
            if refresh_host {
                let host = process::find_host(&record.cwd, input.hook_event_name == "UserPromptSubmit");
                if host.hwnd != 0 || host.pid != 0 {
                    record.host_hwnd = host.hwnd;
                    record.host_pid = host.pid;
                    record.host_exe = host.exe;
                }
            }
            if let Err(error) = store::write_record(&dir, &record) {
                super::log(format!("activity hook: unable to write record: {error}"));
            }
        }
        Update::Delete => store::delete_record(&dir, &input.session_id),
        Update::Skip => {}
    }
    0
}

fn read_stdin(timeout: Duration) -> Option<String> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut text = String::new();
        let _ = std::io::stdin().lock().take(4 * 1024 * 1024).read_to_string(&mut text);
        let _ = sender.send(text);
    });
    receiver.recv_timeout(timeout).ok()
}

/// `--activity-demo`: play every state with fake sessions so the badge can
/// be checked without running Claude.
pub fn demo() -> i32 {
    let dir = settings::sessions_dir();
    let record = |id: &str, phase: store::Phase, started: u64| SessionRecord {
        session_id: id.into(),
        phase,
        updated_ms: store::now_ms(),
        turn_started_ms: started,
        cwd: "demo".into(),
        last_event: "demo".into(),
        ..Default::default()
    };
    let start = store::now_ms();
    let steps: &[(&str, store::Phase, u64)] = &[
        ("demo-a", store::Phase::Working, 0),
        ("demo-b", store::Phase::Working, 6),
        ("demo-a", store::Phase::Attention, 4),
        ("demo-a", store::Phase::Working, 3),
        ("demo-a", store::Phase::Done, 12),
    ];
    for (index, (id, phase, seconds)) in steps.iter().enumerate() {
        let _ = store::write_record(&dir, &record(id, *phase, start));
        if index == 1 {
            // The second session only exists to show the "two working" dots.
            std::thread::sleep(Duration::from_secs(*seconds));
            store::delete_record(&dir, "demo-b");
            continue;
        }
        std::thread::sleep(Duration::from_secs((*seconds).max(1)));
    }
    store::delete_record(&dir, "demo-a");
    store::delete_record(&dir, "demo-b");
    0
}
