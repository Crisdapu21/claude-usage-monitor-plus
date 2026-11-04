//! Feature settings and file locations. Kept apart from the monitor's own
//! settings so upstream changes to that file never meet this feature.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::layout::Side;
use super::store::Timing;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ActivitySettings {
    /// Master switch for the badge.
    pub enabled: bool,
    /// Also show the badge while the session's own window is active.
    pub show_when_focused: bool,
    /// Working animation: "bars", "dots", "ring", "pulse" or "spark".
    pub style: String,
    /// "auto", "left" or "right" of the usage widget.
    pub position: String,
    /// Badge size as a percentage of the widget's height (50-150).
    pub size_percent: u32,
    /// Keep the Claude Code hooks in ~/.claude/settings.json pointing at
    /// this executable.
    pub auto_install_hooks: bool,
    /// Minutes the "finished" badge keeps reminding you.
    pub done_minutes: u64,
    /// Minutes without any sign of life before a working session is
    /// considered abandoned.
    pub stale_minutes: u64,
}

impl Default for ActivitySettings {
    fn default() -> Self {
        Self {
            enabled: true,
            show_when_focused: false,
            style: "bars".into(),
            position: "auto".into(),
            size_percent: 100,
            auto_install_hooks: true,
            done_minutes: 30,
            stale_minutes: 10,
        }
    }
}

impl ActivitySettings {
    pub fn style(&self) -> super::paint::Style {
        super::paint::Style::parse(&self.style)
    }

    pub fn side(&self) -> Side {
        Side::parse(&self.position)
    }

    pub fn timing(&self) -> Timing {
        Timing {
            stale_ms: self.stale_minutes.clamp(1, 24 * 60) * 60_000,
            done_ttl_ms: self.done_minutes.clamp(1, 24 * 60) * 60_000,
            ..Timing::default()
        }
    }

    /// Read the settings file, creating it with defaults the first time so
    /// there is something to edit.
    pub fn load_or_create(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
            Err(_) => {
                let settings = Self::default();
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if let Ok(json) = serde_json::to_string_pretty(&settings) {
                    let _ = std::fs::write(path, json + "\n");
                }
                settings
            }
        }
    }
}

/// `%LOCALAPPDATA%\ClaudeCodeUsageMonitor-Activity`
pub fn base_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("ClaudeCodeUsageMonitor-Activity")
}

pub fn sessions_dir() -> PathBuf {
    base_dir().join("sessions")
}

pub fn settings_path() -> PathBuf {
    base_dir().join("settings.json")
}

/// Claude Code's user settings file, honouring `CLAUDE_CONFIG_DIR`.
pub fn claude_settings_path() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|dir| !dir.is_empty()) {
        return Some(PathBuf::from(dir).join("settings.json"));
    }
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(|home| PathBuf::from(home).join(".claude").join("settings.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_files_keep_defaults_for_missing_keys() {
        let settings: ActivitySettings =
            serde_json::from_str(r#"{ "position": "right", "done_minutes": 5 }"#).unwrap();
        assert!(settings.enabled);
        assert_eq!(settings.side(), Side::After);
        assert_eq!(settings.timing().done_ttl_ms, 5 * 60_000);
    }
}
