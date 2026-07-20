use crate::types::ProviderId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ChatProvider {
    Claude,
    Codex,
    Antigravity,
    Kiro,
    OpenCode,
    Zcode,
}

impl ChatProvider {
    /// Every provider in scan order.
    pub const ALL: &'static [ChatProvider] = &[
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

    pub fn id(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Antigravity => "antigravity",
            Self::Kiro => "kiro",
            Self::OpenCode => "opencode",
            Self::Zcode => "zcode",
        }
    }

    pub fn color(self) -> egui::Color32 {
        match self {
            Self::Claude => ProviderId::Claude.color(),
            Self::Codex => ProviderId::Codex.color(),
            Self::Antigravity => ProviderId::Antigravity.color(),
            Self::Kiro => ProviderId::Kiro.color(),
            Self::OpenCode => ProviderId::OpenCode.color(),
            Self::Zcode => ProviderId::Zcode.color(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChatSession {
    pub id: String,
    pub title: String,
    pub provider: ChatProvider,
    pub project_path: String,
    pub created_at: Option<String>,
    pub updated_at: String,
    pub source_path: Option<PathBuf>,
    pub source_kind: ChatSourceKind,
    pub turn_count: usize,
    pub size_bytes: u64,
    pub imported: bool,
    pub trash_manifest: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChatSourceKind {
    Jsonl,
    JsonlDir,
    ImportedArchive,
    KiroCli,
    OpenCodeDb,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatArchive {
    pub schema_version: u32,
    pub source_provider: ChatProvider,
    pub source_session_id: String,
    pub title: String,
    pub project_path: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub messages: Vec<ChatMessage>,
    pub tool_calls: Vec<ChatToolCall>,
    pub raw_events: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub timestamp: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatToolCall {
    pub name: String,
    pub timestamp: Option<String>,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatZipManifest {
    pub schema_version: u32,
    pub exported_at_unix: u64,
    pub entries: Vec<ChatZipManifestEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatZipManifestEntry {
    pub provider: ChatProvider,
    pub session_id: String,
    pub title: String,
    pub project_path: String,
    pub archive_path: String,
}

#[derive(Debug, Clone, Default)]
pub struct BatchReport {
    pub ok: usize,
    pub failed: usize,
}
