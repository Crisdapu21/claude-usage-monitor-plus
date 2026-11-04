//! Keeps this executable registered as a Claude Code hook.
//!
//! Claude Code's `settings.json` belongs to the user, so edits here are
//! surgical: key order is preserved, only hook entries that carry our marker
//! argument are touched, and the original file is backed up once.

use std::fmt;
use std::path::Path;

use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq, Serializer};
use serde::{Deserialize, Serialize};

/// Argument that marks a hook entry as ours.
pub const HOOK_ARG: &str = "--activity-hook";

/// Events the badge listens to, with the matcher each one needs.
const EVENTS: &[(&str, Option<&str>)] = &[
    ("UserPromptSubmit", None),
    ("PostToolUse", Some("*")),
    ("PostToolUseFailure", Some("*")),
    ("PermissionRequest", Some("*")),
    ("Notification", None),
    ("Stop", None),
    ("StopFailure", None),
    ("SessionEnd", None),
];

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Unchanged,
    Updated,
}

/// Point every hook at `exe`, adding them when missing.
pub fn install(settings_path: &Path, exe: &str) -> Result<Outcome, String> {
    edit(settings_path, |root| {
        remove_ours(root)?;
        add_ours(root, exe)
    })
}

/// Remove every hook entry this feature added.
pub fn uninstall(settings_path: &Path) -> Result<Outcome, String> {
    if !settings_path.exists() {
        return Ok(Outcome::Unchanged);
    }
    edit(settings_path, remove_ours)
}

/// True when the settings already route hooks to `exe`.
#[cfg(test)]
pub fn is_installed(settings_path: &Path, exe: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(settings_path) else {
        return false;
    };
    let Ok(mut root) = serde_json::from_str::<Json>(&text) else {
        return false;
    };
    let before = root.clone();
    if remove_ours(&mut root).and_then(|_| add_ours(&mut root, exe)).is_err() {
        return false;
    }
    root == before
}

fn edit(settings_path: &Path, change: impl FnOnce(&mut Json) -> Result<(), String>) -> Result<Outcome, String> {
    let existing = match std::fs::read_to_string(settings_path) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("No se pudo leer {}: {error}", settings_path.display())),
    };
    let mut root = match existing.as_deref() {
        Some(text) if !text.trim().is_empty() => serde_json::from_str::<Json>(text).map_err(|error| {
            format!(
                "{} no es JSON válido ({error}); no se modificó.",
                settings_path.display()
            )
        })?,
        _ => Json::Object(Vec::new()),
    };
    let before = root.clone();
    change(&mut root)?;
    if root == before {
        return Ok(Outcome::Unchanged);
    }

    if let Some(parent) = settings_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    if existing.is_some() {
        let backup = settings_path.with_extension("json.ccum-activity.bak");
        if !backup.exists() {
            let _ = std::fs::copy(settings_path, &backup);
        }
    }
    let mut text = serde_json::to_string_pretty(&root).map_err(|error| error.to_string())?;
    text.push('\n');
    let temp = settings_path.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&temp, text).map_err(|error| error.to_string())?;
    std::fs::rename(&temp, settings_path).map_err(|error| {
        let _ = std::fs::remove_file(&temp);
        format!("No se pudo guardar {}: {error}", settings_path.display())
    })?;
    Ok(Outcome::Updated)
}

fn our_entry(exe: &str) -> Json {
    Json::Object(vec![
        ("type".into(), Json::String("command".into())),
        ("command".into(), Json::String(exe.into())),
        ("args".into(), Json::Array(vec![Json::String(HOOK_ARG.into())])),
        ("async".into(), Json::Bool(true)),
    ])
}

fn is_ours(hook: &Json) -> bool {
    let args_mark = hook
        .get("args")
        .and_then(Json::as_array)
        .and_then(|args| args.first())
        .and_then(Json::as_str)
        == Some(HOOK_ARG);
    let command_mark = hook
        .get("command")
        .and_then(Json::as_str)
        .is_some_and(|command| command.contains(HOOK_ARG));
    args_mark || command_mark
}

fn remove_ours(root: &mut Json) -> Result<(), String> {
    let Json::Object(entries) = root else {
        return Err("settings.json no contiene un objeto JSON.".into());
    };
    let Some((_, hooks)) = entries.iter_mut().find(|(key, _)| key == "hooks") else {
        return Ok(());
    };
    let Json::Object(events) = hooks else {
        return Ok(());
    };
    for (_, groups) in events.iter_mut() {
        let Json::Array(groups) = groups else { continue };
        groups.retain_mut(|group| {
            let Some(Json::Array(list)) = group.get_mut("hooks") else {
                return true;
            };
            let had = list.len();
            list.retain(|hook| !is_ours(hook));
            !(had > 0 && list.is_empty())
        });
    }
    events.retain(|(_, groups)| !matches!(groups, Json::Array(list) if list.is_empty()));
    if events.is_empty() {
        entries.retain(|(key, _)| key != "hooks");
    }
    Ok(())
}

fn add_ours(root: &mut Json, exe: &str) -> Result<(), String> {
    let Json::Object(entries) = root else {
        return Err("settings.json no contiene un objeto JSON.".into());
    };
    if !entries.iter().any(|(key, _)| key == "hooks") {
        entries.push(("hooks".into(), Json::Object(Vec::new())));
    }
    let Some((_, Json::Object(events))) = entries.iter_mut().find(|(key, _)| key == "hooks") else {
        return Err("La clave \"hooks\" de settings.json no es un objeto; no se modificó.".into());
    };
    for (event, matcher) in EVENTS {
        if !events.iter().any(|(key, _)| key == event) {
            events.push(((*event).into(), Json::Array(Vec::new())));
        }
        let Some((_, Json::Array(groups))) = events.iter_mut().find(|(key, _)| key == event) else {
            continue;
        };
        let mut group = Vec::new();
        if let Some(matcher) = matcher {
            group.push(("matcher".to_string(), Json::String((*matcher).into())));
        }
        group.push(("hooks".to_string(), Json::Array(vec![our_entry(exe)])));
        groups.push(Json::Object(group));
    }
    Ok(())
}

/// JSON value that keeps object keys in file order.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(serde_json::Number),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn get_mut(&mut self, key: &str) -> Option<&mut Json> {
        match self {
            Json::Object(entries) => entries.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn as_array(&self) -> Option<&Vec<Json>> {
        match self {
            Json::Array(items) => Some(items),
            _ => None,
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(text) => Some(text),
            _ => None,
        }
    }
}

impl Serialize for Json {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Json::Null => serializer.serialize_unit(),
            Json::Bool(value) => serializer.serialize_bool(*value),
            Json::Number(value) => value.serialize(serializer),
            Json::String(value) => serializer.serialize_str(value),
            Json::Array(items) => {
                let mut seq = serializer.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
            Json::Object(entries) => {
                let mut map = serializer.serialize_map(Some(entries.len()))?;
                for (key, value) in entries {
                    map.serialize_entry(key, value)?;
                }
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for Json {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct JsonVisitor;

        impl<'de> Visitor<'de> for JsonVisitor {
            type Value = Json;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("any JSON value")
            }

            fn visit_unit<E: de::Error>(self) -> Result<Json, E> {
                Ok(Json::Null)
            }

            fn visit_none<E: de::Error>(self) -> Result<Json, E> {
                Ok(Json::Null)
            }

            fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Json, D::Error> {
                Json::deserialize(d)
            }

            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Json, E> {
                Ok(Json::Bool(v))
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Json, E> {
                Ok(Json::Number(v.into()))
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Json, E> {
                Ok(Json::Number(v.into()))
            }

            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Json, E> {
                Ok(serde_json::Number::from_f64(v).map_or(Json::Null, Json::Number))
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Json, E> {
                Ok(Json::String(v.to_owned()))
            }

            fn visit_string<E: de::Error>(self, v: String) -> Result<Json, E> {
                Ok(Json::String(v))
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Json, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element()? {
                    items.push(item);
                }
                Ok(Json::Array(items))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Json, A::Error> {
                let mut entries: Vec<(String, Json)> = Vec::new();
                while let Some((key, value)) = map.next_entry::<String, Json>()? {
                    // Like JSON.parse: a repeated key keeps the last value.
                    if let Some(slot) = entries.iter_mut().find(|(k, _)| *k == key) {
                        slot.1 = value;
                    } else {
                        entries.push((key, value));
                    }
                }
                Ok(Json::Object(entries))
            }
        }

        deserializer.deserialize_any(JsonVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = r"C:\Utilidades\claude-code-usage-monitor.exe";

    fn temp_file(name: &str, contents: Option<&str>) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ccum-install-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        if let Some(contents) = contents {
            std::fs::write(&path, contents).unwrap();
        }
        path
    }

    #[test]
    fn install_preserves_user_settings_and_order() {
        let original = r#"{
  "model": "opus",
  "permissions": { "allow": ["Bash(npm test)"] },
  "hooks": {
    "Stop": [ { "hooks": [ { "type": "command", "command": "say listo" } ] } ]
  },
  "alwaysThinkingEnabled": true
}"#;
        let path = temp_file("preserve", Some(original));
        assert_eq!(install(&path, EXE).unwrap(), Outcome::Updated);
        let text = std::fs::read_to_string(&path).unwrap();
        let model = text.find("\"model\"").unwrap();
        let permissions = text.find("\"permissions\"").unwrap();
        let thinking = text.find("\"alwaysThinkingEnabled\"").unwrap();
        assert!(model < permissions && permissions < thinking, "{text}");
        assert!(text.contains("say listo"));
        assert!(text.contains(HOOK_ARG));
        assert!(is_installed(&path, EXE));
        assert!(path.with_extension("json.ccum-activity.bak").exists());

        // Running again is a no-op; a moved exe rewrites only our entries.
        assert_eq!(install(&path, EXE).unwrap(), Outcome::Unchanged);
        assert_eq!(install(&path, r"D:\otro\monitor.exe").unwrap(), Outcome::Updated);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("Utilidades"));
        assert_eq!(text.matches(HOOK_ARG).count(), EVENTS.len());

        // Uninstall restores the user's own hooks only.
        assert_eq!(uninstall(&path).unwrap(), Outcome::Updated);
        let root: Json = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let expected: Json = serde_json::from_str(original).unwrap();
        assert_eq!(root, expected);
    }

    #[test]
    fn install_creates_missing_file_and_rejects_invalid_json() {
        let path = temp_file("missing", None);
        assert_eq!(install(&path, EXE).unwrap(), Outcome::Updated);
        assert!(is_installed(&path, EXE));
        assert_eq!(uninstall(&path).unwrap(), Outcome::Updated);
        assert_eq!(std::fs::read_to_string(&path).unwrap().trim(), "{}");

        let broken = temp_file("broken", Some("{ \"model\": "));
        assert!(install(&broken, EXE).is_err());
        assert_eq!(std::fs::read_to_string(&broken).unwrap(), "{ \"model\": ");
    }
}
