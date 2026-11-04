//! Claude activity badge.
//!
//! While a Claude Code session (terminal, VS Code, the Claude desktop app)
//! works in a window you are not looking at, an animated badge next to the
//! usage widget shows that Claude is working; when it needs you or finishes,
//! the badge says so. Click the badge to jump back to that window.
//!
//! This module is an addition kept on top of upstream Claude Code Usage
//! Monitor. It is self-contained on purpose: upstream code only calls
//! `handle_cli_mode` and `start` from `main.rs`, so syncing with upstream
//! never has to merge into the widget's own code.

mod hook;
mod install;
mod layout;
mod overlay;
mod paint;
mod process;
mod settings;
mod store;

use windows::core::PCWSTR;
use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_ICONINFORMATION, MB_OK};

/// Command-line entry points. Returns an exit code when the process should
/// stop after handling one.
pub fn handle_cli_mode(args: &[String]) -> Option<i32> {
    let flag = args.get(1)?.as_str();
    match flag {
        install::HOOK_ARG => Some(hook::run()),
        "--activity-demo" => Some(hook::demo()),
        "--activity-reset" => {
            let dir = settings::sessions_dir();
            for (_, record) in store::list_records(&dir) {
                store::delete_record(&dir, &record.session_id);
            }
            Some(0)
        }
        "--activity-install-hooks" => Some(match ensure_hooks(true) {
            Ok(message) => {
                message_box(&message, false);
                0
            }
            Err(error) => {
                message_box(&error, true);
                1
            }
        }),
        "--activity-uninstall-hooks" => {
            let Some(path) = settings::claude_settings_path() else {
                message_box("No se encontró la carpeta de usuario.", true);
                return Some(1);
            };
            Some(match install::uninstall(&path) {
                Ok(_) => {
                    message_box(
                        &format!(
                            "Se quitaron los hooks de actividad de {}.\n\nReinicia tus sesiones de Claude Code para que tome el cambio.",
                            path.display()
                        ),
                        false,
                    );
                    0
                }
                Err(error) => {
                    message_box(&error, true);
                    1
                }
            })
        }
        _ => None,
    }
}

/// Start the badge on its own thread. Safe to call before the monitor's
/// single-instance check: a duplicate process exits before it does anything.
pub fn start() {
    let _ = std::thread::Builder::new()
        .name("activity-badge".into())
        .spawn(overlay::run);
}

/// Register (or re-point) the Claude Code hooks at this executable.
fn ensure_hooks(explicit: bool) -> Result<String, String> {
    let current = settings::ActivitySettings::load_or_create(&settings::settings_path());
    if !explicit && !current.auto_install_hooks {
        return Ok(String::new());
    }
    let path = settings::claude_settings_path().ok_or("No se encontró la carpeta de usuario.")?;
    let exe = std::env::current_exe()
        .map_err(|error| format!("No se pudo ubicar el ejecutable: {error}"))?
        .to_string_lossy()
        .into_owned();
    let result = install::install(&path, &exe);
    match &result {
        Ok(install::Outcome::Updated) => log(format!("activity: hooks installed in {}", path.display())),
        Ok(install::Outcome::Unchanged) => log("activity: hooks already installed"),
        Err(error) => log(format!("activity: hook install failed: {error}")),
    }
    result.map(|outcome| {
        let what = match outcome {
            install::Outcome::Updated => "Hooks de actividad instalados",
            install::Outcome::Unchanged => "Los hooks de actividad ya estaban instalados",
        };
        format!(
            "{what} en {}.\n\nLas sesiones de Claude Code que ya estaban abiertas deben reiniciarse (en VS Code: Developer: Reload Window).",
            path.display()
        )
    })
}

fn log(message: impl AsRef<str>) {
    crate::diagnose::log(message);
}

fn message_box(text: &str, error: bool) {
    let title: Vec<u16> = "Claude Code Usage Monitor"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let body: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(body.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | if error { MB_ICONERROR } else { MB_ICONINFORMATION },
        );
    }
}
