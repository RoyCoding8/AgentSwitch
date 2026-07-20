use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookLoc {
    /// Slash-separated object path from the config root to the event maps,
    /// e.g. "hooks" for Claude-style files or "hooks/events" for ZCode.
    pub section: String,
    pub event: String,
    pub order: usize,
    pub hook_name: String,
    pub fingerprint: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ItemKind {
    Skill,
    Hook,
    Rule,
    Agent,
    Mcp,
    Plugin,
    InstructionFile,
    SteeringRule,
    Spec,
}

impl ItemKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Skill => "Skills",
            Self::Hook => "Hooks",
            Self::Rule => "Rules",
            Self::Agent => "Agents",
            Self::Mcp => "MCP",
            Self::Plugin => "Plugins",
            Self::InstructionFile => "Files",
            Self::SteeringRule => "Steering",
            Self::Spec => "Specs",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemState {
    Enabled,
    Disabled,
}

impl ItemState {
    pub fn is_enabled(self) -> bool {
        self == Self::Enabled
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderId {
    Claude,
    Codex,
    Antigravity,
    Kiro,
    OpenCode,
    Zcode,
}

impl ProviderId {
    pub const ALL: &[ProviderId] = &[
        Self::Claude,
        Self::Codex,
        Self::Antigravity,
        Self::Kiro,
        Self::OpenCode,
        Self::Zcode,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex CLI",
            Self::Antigravity => "Antigravity CLI",
            Self::Kiro => "Kiro",
            Self::OpenCode => "OpenCode",
            Self::Zcode => "ZCode",
        }
    }
    pub fn color(self) -> egui::Color32 {
        match self {
            Self::Claude => egui::Color32::from_rgb(0xD9, 0x77, 0x57),
            Self::Codex => egui::Color32::from_rgb(0x10, 0xA3, 0x7F),
            Self::Antigravity => egui::Color32::from_rgb(0xF4, 0xB4, 0x00),
            Self::Kiro => egui::Color32::from_rgb(0x7B, 0x61, 0xFF),
            Self::OpenCode => egui::Color32::from_rgb(0xFF, 0x6B, 0x35),
            Self::Zcode => egui::Color32::from_rgb(0x3D, 0xB4, 0xE4),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    Project,
    Global,
}

#[derive(Debug, Clone)]
pub enum ToggleSpec {
    JsonFlag {
        section: String,
        name: String,
        flag: String,
        enabled_value: bool,
        disabled_value: bool,
    },
    TomlFlag {
        section: String,
        name: String,
        flag: String,
        enabled_value: bool,
        disabled_value: bool,
    },
    StringLists {
        path: PathBuf,
        enabled_key: String,
        disabled_key: String,
        name: String,
    },
    JsonStash {
        section: String,
        name: String,
    },
}

#[derive(Debug, Clone)]
pub struct ConfigItem {
    pub name: String,
    pub kind: ItemKind,
    pub state: ItemState,
    pub path: PathBuf,
    pub provider: ProviderId,
    pub editable: bool,
    pub hook_loc: Option<HookLoc>,
    pub toggle_spec: Option<ToggleSpec>,
    pub detail: Option<String>,
}

impl ConfigItem {
    pub fn new(
        name: impl Into<String>,
        kind: ItemKind,
        path: PathBuf,
        provider: ProviderId,
    ) -> Self {
        let mut n: String = name.into();
        let editable = matches!(
            kind,
            ItemKind::InstructionFile | ItemKind::Rule | ItemKind::SteeringRule
        ) && kind != ItemKind::Plugin;
        let state = if path.extension() == Some(OsStr::new("disabled"))
            || path.parent().and_then(|parent| parent.extension()) == Some(OsStr::new("disabled"))
        {
            if let Some(stripped) = n.strip_suffix(".disabled") {
                n = stripped
                    .strip_prefix('.')
                    .filter(|rest| !rest.is_empty())
                    .unwrap_or(stripped)
                    .to_string();
            }
            ItemState::Disabled
        } else {
            ItemState::Enabled
        };
        Self {
            name: n,
            kind,
            state,
            path,
            provider,
            editable,
            hook_loc: None,
            toggle_spec: None,
            detail: None,
        }
    }
    pub fn disabled_path(&self) -> PathBuf {
        if let (Some(parent), Some(file_name)) = (self.path.parent(), self.path.file_name()) {
            if let Some(parent_name) = parent.file_name().and_then(|name| name.to_str()) {
                if matches!(
                    parent_name,
                    "skills" | "agents" | "specs" | "rules" | "steering"
                ) {
                    let mut disabled_parent = parent.to_path_buf();
                    disabled_parent.set_extension("disabled");
                    return disabled_parent.join(file_name);
                }
            }
        }

        let mut disabled = self.path.as_os_str().to_os_string();
        disabled.push(".disabled");
        PathBuf::from(disabled)
    }

    pub fn enabled_path(&self) -> PathBuf {
        if let (Some(parent), Some(file_name)) = (self.path.parent(), self.path.file_name()) {
            if parent.extension() == Some(OsStr::new("disabled")) {
                if let Some(base) = parent.file_stem().and_then(|name| name.to_str()) {
                    if matches!(base, "skills" | "agents" | "specs" | "rules" | "steering") {
                        let mut enabled_parent = parent.to_path_buf();
                        enabled_parent.set_extension("");
                        return enabled_parent.join(file_name);
                    }
                }
            }
        }

        if self.path.extension() == Some(OsStr::new("disabled")) {
            self.path.with_extension("")
        } else {
            self.path.clone()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterKind {
    All,
    Specific(ItemKind),
}

/// Look up the first matching key in a JSON value and return it as &str.
pub fn str_field<'a>(value: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(|v| v.as_str()))
}

/// Parse a string as JSON, falling back to a JSON string literal.
pub fn parse_json_or_string(raw: &str) -> serde_json::Value {
    serde_json::from_str(raw).unwrap_or_else(|_| serde_json::Value::String(raw.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_toggle_paths_round_trip_without_string_reconstruction() {
        let item = ConfigItem::new(
            "CLAUDE.md",
            ItemKind::InstructionFile,
            PathBuf::from("project").join("CLAUDE.md"),
            ProviderId::Claude,
        );
        let disabled = item.disabled_path();
        assert_eq!(
            disabled,
            PathBuf::from("project").join("CLAUDE.md.disabled")
        );

        let disabled_item = ConfigItem::new(
            "CLAUDE.md.disabled",
            ItemKind::InstructionFile,
            disabled,
            ProviderId::Claude,
        );
        assert_eq!(disabled_item.enabled_path(), item.path);
    }

    #[test]
    fn directory_toggle_paths_round_trip() {
        let item = ConfigItem::new(
            "review",
            ItemKind::Skill,
            PathBuf::from(".claude").join("skills").join("review"),
            ProviderId::Claude,
        );
        let disabled = item.disabled_path();
        assert_eq!(
            disabled,
            PathBuf::from(".claude")
                .join("skills.disabled")
                .join("review")
        );

        let disabled_item =
            ConfigItem::new("review", ItemKind::Skill, disabled, ProviderId::Claude);
        assert_eq!(disabled_item.enabled_path(), item.path);
    }

    #[cfg(unix)]
    #[test]
    fn file_toggle_paths_preserve_non_utf8_names() {
        use std::os::unix::ffi::OsStringExt;

        let name = std::ffi::OsString::from_vec(vec![b'a', 0x80]);
        let path = PathBuf::from("project").join(&name);
        let item = ConfigItem::new(
            "non-utf8",
            ItemKind::InstructionFile,
            path.clone(),
            ProviderId::Claude,
        );
        let disabled = item.disabled_path();
        let disabled_item = ConfigItem::new(
            "non-utf8.disabled",
            ItemKind::InstructionFile,
            disabled,
            ProviderId::Claude,
        );
        assert_eq!(disabled_item.enabled_path(), path);
    }
}
