use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookLoc {
    pub event: String,
    pub index: usize,
    pub hook_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ItemKind {
    Skill,
    Hook,
    Rule,
    Agent,
    Mcp,
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
            Self::InstructionFile => "Files",
            Self::SteeringRule => "Steering",
            Self::Spec => "Specs",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    Gemini,
    Kiro,
    OpenCode,
}

impl ProviderId {
    pub const ALL: &[ProviderId] = &[
        Self::Claude,
        Self::Codex,
        Self::Gemini,
        Self::Kiro,
        Self::OpenCode,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex CLI",
            Self::Gemini => "Gemini CLI",
            Self::Kiro => "Kiro",
            Self::OpenCode => "OpenCode",
        }
    }
    pub fn color(self) -> egui::Color32 {
        match self {
            Self::Claude => egui::Color32::from_rgb(0xD9, 0x77, 0x57),
            Self::Codex => egui::Color32::from_rgb(0x10, 0xA3, 0x7F),
            Self::Gemini => egui::Color32::from_rgb(0x42, 0x85, 0xF4),
            Self::Kiro => egui::Color32::from_rgb(0x7B, 0x61, 0xFF),
            Self::OpenCode => egui::Color32::from_rgb(0xFF, 0x6B, 0x35),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    Project,
    Global,
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
    pub detail: Option<String>,
}

impl ConfigItem {
    pub fn new(
        name: impl Into<String>,
        kind: ItemKind,
        path: PathBuf,
        provider: ProviderId,
    ) -> Self {
        let editable = matches!(
            kind,
            ItemKind::InstructionFile | ItemKind::Rule | ItemKind::SteeringRule
        );
        let state = if path.extension().and_then(|e| e.to_str()) == Some("disabled")
            || path.to_string_lossy().contains(".disabled")
        {
            ItemState::Disabled
        } else {
            ItemState::Enabled
        };
        Self {
            name: name.into(),
            kind,
            state,
            path,
            provider,
            editable,
            hook_loc: None,
            detail: None,
        }
    }
    pub fn disabled_path(&self) -> PathBuf {
        PathBuf::from(format!("{}.disabled", self.path.display()))
    }
    pub fn enabled_path(&self) -> PathBuf {
        let s = self.path.to_string_lossy();
        if let Some(base) = s.strip_suffix(".disabled") {
            PathBuf::from(base)
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
