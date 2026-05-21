use crate::types::ProviderId;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    env,
    fs::{self, File},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use walkdir::WalkDir;
use zip::{read::ZipArchive, write::SimpleFileOptions, CompressionMethod, ZipWriter};

const ARCHIVE_VERSION: u32 = 1;
const ARCHIVE_EXT: &str = "agentswitch-chat.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ChatProvider {
    Claude,
    Codex,
    Gemini,
    Antigravity,
    Kiro,
}

impl ChatProvider {
    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex CLI",
            Self::Gemini => "Gemini CLI",
            Self::Antigravity => "Antigravity CLI",
            Self::Kiro => "Kiro",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Gemini => "gemini",
            Self::Antigravity => "antigravity",
            Self::Kiro => "kiro",
        }
    }

    pub fn color(self) -> egui::Color32 {
        match self {
            Self::Claude => ProviderId::Claude.color(),
            Self::Codex => ProviderId::Codex.color(),
            Self::Gemini => ProviderId::Gemini.color(),
            Self::Antigravity => ProviderId::Antigravity.color(),
            Self::Kiro => ProviderId::Kiro.color(),
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
struct DeleteManifest {
    schema_version: u32,
    provider: ChatProvider,
    session_id: String,
    title: Option<String>,
    project_path: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
    turn_count: Option<usize>,
    size_bytes: Option<u64>,
    imported: Option<bool>,
    original_path: Option<PathBuf>,
    trashed_path: Option<PathBuf>,
    source_kind: Option<ChatSourceKind>,
    deleted_at_unix: u64,
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

#[derive(Default)]
struct SessionMeta {
    id: Option<String>,
    title: Option<String>,
    project_path: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
    turn_count: usize,
}

pub fn scan_all(workspace: &Path) -> Vec<ChatSession> {
    let mut sessions = Vec::new();
    sessions.extend(scan_claude());
    sessions.extend(scan_codex());
    sessions.extend(scan_gemini());
    sessions.extend(scan_antigravity());
    sessions.extend(scan_kiro(workspace));
    sessions.extend(scan_imported());
    sessions.sort_by(|a, b| {
        a.provider
            .cmp(&b.provider)
            .then(a.project_path.cmp(&b.project_path))
            .then(b.updated_at.cmp(&a.updated_at))
            .then(a.title.cmp(&b.title))
    });
    sessions
}

pub fn scan_trash() -> Vec<ChatSession> {
    let root = trash_dir();
    if !root.is_dir() {
        return vec![];
    }
    let mut sessions: Vec<_> = WalkDir::new(root)
        .max_depth(3)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().to_string_lossy().ends_with(".delete.json"))
        .filter_map(|e| trashed_session(e.path()).ok())
        .collect();
    sessions.sort_by(|a, b| {
        a.provider
            .cmp(&b.provider)
            .then(a.project_path.cmp(&b.project_path))
            .then(b.updated_at.cmp(&a.updated_at))
            .then(a.title.cmp(&b.title))
    });
    sessions
}

pub fn load_archive(session: &ChatSession) -> Result<ChatArchive> {
    match session.source_kind {
        ChatSourceKind::ImportedArchive => {
            let path = session
                .source_path
                .as_ref()
                .ok_or_else(|| anyhow!("imported chat has no file path"))?;
            let archive: ChatArchive = serde_json::from_str(&fs::read_to_string(path)?)?;
            validate_archive(&archive)?;
            Ok(archive)
        }
        ChatSourceKind::Jsonl | ChatSourceKind::JsonlDir => load_jsonl_archive(session),
        ChatSourceKind::KiroCli => load_kiro_archive(session),
    }
}

pub fn export_session(session: &ChatSession, target: &Path) -> Result<()> {
    let archive = load_archive(session)?;
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(target, serde_json::to_string_pretty(&archive)?)?;
    Ok(())
}

pub fn export_sessions_zip(sessions: &[ChatSession], target: &Path) -> Result<BatchReport> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = File::create(target)?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let mut report = BatchReport::default();
    let mut entries = Vec::new();
    for session in sessions {
        match load_archive(session) {
            Ok(archive) => {
                let name = format!(
                    "chats/{}-{}.{}",
                    session.provider.id(),
                    safe_file_stem(&format!("{}-{}", session.id, session.title)),
                    ARCHIVE_EXT
                );
                zip.start_file(&name, options)?;
                zip.write_all(serde_json::to_string_pretty(&archive)?.as_bytes())?;
                entries.push(ChatZipManifestEntry {
                    provider: session.provider,
                    session_id: session.id.clone(),
                    title: session.title.clone(),
                    project_path: session.project_path.clone(),
                    archive_path: name,
                });
                report.ok += 1;
            }
            Err(_) => report.failed += 1,
        }
    }
    let manifest = ChatZipManifest {
        schema_version: ARCHIVE_VERSION,
        exported_at_unix: unix_now(),
        entries,
    };
    zip.start_file("manifest.json", options)?;
    zip.write_all(serde_json::to_string_pretty(&manifest)?.as_bytes())?;
    zip.finish()?;
    Ok(report)
}

pub fn import_archive(path: &Path, project_dir: Option<&Path>) -> Result<PathBuf> {
    let mut archive: ChatArchive = serde_json::from_str(&fs::read_to_string(path)?)?;
    validate_archive(&archive)?;
    if let Some(dir) = project_dir {
        archive.project_path = dir.to_string_lossy().to_string();
    }
    let dir = imports_dir();
    fs::create_dir_all(&dir)?;
    let base = safe_file_stem(&format!(
        "{}-{}",
        archive.source_provider.id(),
        archive.title
    ));
    let mut target = dir.join(format!("{base}.{ARCHIVE_EXT}"));
    let mut n = 2usize;
    while target.exists() {
        target = dir.join(format!("{base}-{n}.{ARCHIVE_EXT}"));
        n += 1;
    }
    fs::write(&target, serde_json::to_string_pretty(&archive)?)?;
    Ok(target)
}

pub fn import_zip(path: &Path, project_dir: Option<&Path>) -> Result<BatchReport> {
    let file = File::open(path)?;
    let mut zip = ZipArchive::new(file)?;
    let dir = imports_dir();
    fs::create_dir_all(&dir)?;
    let mut report = BatchReport::default();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let name = entry.name().to_string();
        if !name.ends_with(ARCHIVE_EXT) {
            continue;
        }
        let mut buf = String::new();
        std::io::Read::read_to_string(&mut entry, &mut buf)?;
        let mut archive: ChatArchive = match serde_json::from_str(&buf) {
            Ok(a) => a,
            Err(_) => {
                report.failed += 1;
                continue;
            }
        };
        if validate_archive(&archive).is_err() {
            report.failed += 1;
            continue;
        }
        if let Some(d) = project_dir {
            archive.project_path = d.to_string_lossy().to_string();
        }
        let base = safe_file_stem(&format!(
            "{}-{}",
            archive.source_provider.id(),
            archive.title
        ));
        let mut target = dir.join(format!("{base}.{ARCHIVE_EXT}"));
        let mut n = 2usize;
        while target.exists() {
            target = dir.join(format!("{base}-{n}.{ARCHIVE_EXT}"));
            n += 1;
        }
        fs::write(&target, serde_json::to_string_pretty(&archive)?)?;
        report.ok += 1;
    }
    Ok(report)
}

pub fn exports_dir() -> PathBuf {
    data_dir().join("chats").join("exports")
}

pub fn soft_delete(session: &ChatSession, workspace: &Path) -> Result<()> {
    let _ = workspace;
    if session.source_kind == ChatSourceKind::KiroCli {
        return soft_delete_kiro_session(session);
    }

    let source = session
        .source_path
        .as_ref()
        .ok_or_else(|| anyhow!("chat has no file path to delete"))?;
    let trash_dir = trash_dir().join(session.provider.id());
    fs::create_dir_all(&trash_dir)?;
    let stem = safe_file_stem(&format!("{}-{}", session.id, session.title));
    let ext = source
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("chat");
    let mut target = trash_dir.join(format!("{stem}.{ext}"));
    let mut n = 2usize;
    while target.exists() {
        target = trash_dir.join(format!("{stem}-{n}.{ext}"));
        n += 1;
    }
    move_path(source, &target)?;
    let manifest = DeleteManifest {
        schema_version: ARCHIVE_VERSION,
        provider: session.provider,
        session_id: session.id.clone(),
        title: Some(session.title.clone()),
        project_path: Some(session.project_path.clone()),
        created_at: session.created_at.clone(),
        updated_at: Some(session.updated_at.clone()),
        turn_count: Some(session.turn_count),
        size_bytes: Some(session.size_bytes),
        imported: Some(session.imported),
        original_path: Some(source.clone()),
        trashed_path: Some(target.clone()),
        source_kind: Some(session.source_kind),
        deleted_at_unix: unix_now(),
    };
    fs::write(
        target.with_extension("delete.json"),
        serde_json::to_string_pretty(&manifest)?,
    )?;
    Ok(())
}

fn soft_delete_kiro_session(session: &ChatSession) -> Result<()> {
    let source = session
        .source_path
        .as_ref()
        .ok_or_else(|| anyhow!("Kiro chat has no file path"))?;
    let meta_path = kiro_meta_path(source)?;
    let trash_dir = trash_dir().join(session.provider.id());
    fs::create_dir_all(&trash_dir)?;
    let stem = safe_file_stem(&format!("{}-{}", session.id, session.title));
    let mut target = trash_dir.join(&stem);
    let mut n = 2usize;
    while target.exists() {
        target = trash_dir.join(format!("{stem}-{n}"));
        n += 1;
    }
    fs::create_dir_all(&target)?;
    let base = meta_path.with_extension("");
    for ext in ["json", "jsonl", "lock"] {
        let source_file = base.with_extension(ext);
        if source_file.exists() {
            let dest = target.join(
                source_file
                    .file_name()
                    .ok_or_else(|| anyhow!("invalid Kiro session path"))?,
            );
            move_path(&source_file, &dest)?;
        }
    }
    let manifest = DeleteManifest {
        schema_version: ARCHIVE_VERSION,
        provider: session.provider,
        session_id: session.id.clone(),
        title: Some(session.title.clone()),
        project_path: Some(session.project_path.clone()),
        created_at: session.created_at.clone(),
        updated_at: Some(session.updated_at.clone()),
        turn_count: Some(session.turn_count),
        size_bytes: Some(session.size_bytes),
        imported: Some(session.imported),
        original_path: Some(meta_path),
        trashed_path: Some(target.clone()),
        source_kind: Some(session.source_kind),
        deleted_at_unix: unix_now(),
    };
    fs::write(
        target.with_extension("delete.json"),
        serde_json::to_string_pretty(&manifest)?,
    )?;
    Ok(())
}

pub fn restore_from_trash(session: &ChatSession) -> Result<PathBuf> {
    let manifest_path = session
        .trash_manifest
        .as_ref()
        .ok_or_else(|| anyhow!("trash manifest missing"))?;
    let manifest: DeleteManifest = serde_json::from_str(&fs::read_to_string(manifest_path)?)?;
    let source = manifest
        .trashed_path
        .as_ref()
        .ok_or_else(|| anyhow!("trashed path missing"))?;
    let original = manifest
        .original_path
        .as_ref()
        .ok_or_else(|| anyhow!("original path missing"))?;
    if manifest.source_kind == Some(ChatSourceKind::KiroCli) && source.is_dir() {
        return restore_kiro_session(source, original, manifest_path);
    }
    let target = available_restore_path(original);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    move_path(source, &target)?;
    fs::remove_file(manifest_path)?;
    Ok(target)
}

fn restore_kiro_session(source: &Path, original: &Path, manifest: &Path) -> Result<PathBuf> {
    let target_json = available_restore_path(original);
    let target_base = target_json.with_extension("");
    if let Some(parent) = target_json.parent() {
        fs::create_dir_all(parent)?;
    }
    for entry in fs::read_dir(source)?.flatten() {
        let path = entry.path();
        if path.is_file() {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let dest = target_base.with_extension(ext);
            move_path(&path, &dest)?;
        }
    }
    fs::remove_dir_all(source)?;
    fs::remove_file(manifest)?;
    Ok(target_json)
}

pub fn delete_trash_forever(session: &ChatSession) -> Result<()> {
    let manifest_path = session
        .trash_manifest
        .as_ref()
        .ok_or_else(|| anyhow!("trash manifest missing"))?;
    let manifest: DeleteManifest = serde_json::from_str(&fs::read_to_string(manifest_path)?)?;
    if let Some(path) = manifest.trashed_path {
        if path.is_dir() {
            fs::remove_dir_all(path)?;
        } else if path.is_file() {
            fs::remove_file(path)?;
        }
    }
    fs::remove_file(manifest_path)?;
    Ok(())
}

pub fn metadata_matches(session: &ChatSession, query: &str) -> bool {
    let q = query.trim().to_ascii_lowercase();
    if q.is_empty() {
        return true;
    }
    let source = session
        .source_path
        .as_ref()
        .map(|p| p.to_string_lossy())
        .unwrap_or_default();
    let created = session.created_at.as_deref().unwrap_or("");
    let imported = if session.imported {
        "imported"
    } else {
        "local"
    };
    let haystack = format!(
        "{} {} {} {} {} {} {} {}",
        session.title,
        session.provider.label(),
        session.provider.id(),
        session.project_path,
        source,
        created,
        session.updated_at,
        imported
    )
    .to_ascii_lowercase();
    q.split_whitespace().all(|part| haystack.contains(part))
}

pub fn suggested_export_name(session: &ChatSession) -> String {
    format!("{}.{}", safe_file_stem(&session.title), ARCHIVE_EXT)
}

pub fn suggested_zip_export_name() -> String {
    "agentswitch-chats.zip".into()
}

pub fn session_key(session: &ChatSession) -> String {
    format!(
        "{}:{}:{}",
        session.provider.id(),
        session.id,
        session
            .source_path
            .as_ref()
            .map(|p| p.to_string_lossy())
            .unwrap_or_default()
    )
}

fn scan_claude() -> Vec<ChatSession> {
    claude_home()
        .map(|dir| scan_jsonl_root(ChatProvider::Claude, &dir.join("projects"), 8))
        .unwrap_or_default()
}

fn claude_home() -> Option<PathBuf> {
    env::var("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .ok()
        .or_else(|| dirs::home_dir().map(|home| home.join(".claude")))
}

fn scan_codex() -> Vec<ChatSession> {
    let mut roots = Vec::new();
    if let Ok(custom) = env::var("CODEX_HOME") {
        let custom = PathBuf::from(custom);
        roots.push(custom.join("sessions"));
        roots.push(custom.join("archived_sessions"));
    }
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join(".codex").join("sessions"));
        roots.push(home.join(".codex").join("archived_sessions"));
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let titles = codex_titles();
    for root in roots {
        for mut session in scan_jsonl_root(ChatProvider::Codex, &root, 8) {
            if let Some(title) = titles.get(&session.id) {
                session.title = title.clone();
            }
            if let Some(path) = &session.source_path {
                if seen.insert(path.clone()) {
                    out.push(session);
                }
            }
        }
    }
    out
}

fn scan_gemini() -> Vec<ChatSession> {
    let Some(home) = dirs::home_dir() else {
        return vec![];
    };
    let tmp = env::var("GEMINI_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home.join(".gemini"))
        .join("tmp");
    if !tmp.is_dir() {
        return vec![];
    }
    let mut out = Vec::new();
    let Ok(projects) = fs::read_dir(&tmp) else {
        return out;
    };
    for project in projects.flatten().filter(|e| e.path().is_dir()) {
        let chats = project.path().join("chats");
        if let Ok(entries) = fs::read_dir(&chats) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() && is_gemini_session_file(&path) {
                    if let Ok(session) = jsonl_session(ChatProvider::Gemini, &tmp, &path) {
                        if session.turn_count > 0 {
                            out.push(session);
                        }
                    }
                } else if path.is_dir() {
                    let files = jsonl_files_in(&path, 1);
                    if !files.is_empty() {
                        if let Ok(session) =
                            jsonl_dir_session(ChatProvider::Gemini, &tmp, &path, files)
                        {
                            if session.turn_count > 0 {
                                out.push(session);
                            }
                        }
                    }
                }
            }
        }
        if let Ok(entries) = fs::read_dir(project.path()) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() && is_gemini_checkpoint_file(&path) {
                    if let Ok(session) = jsonl_session(ChatProvider::Gemini, &tmp, &path) {
                        if session.turn_count > 0 {
                            out.push(session);
                        }
                    }
                }
            }
        }
    }
    out
}

fn scan_antigravity() -> Vec<ChatSession> {
    let Some(home) = dirs::home_dir() else {
        return vec![];
    };
    let tmp = home.join(".gemini").join("antigravity-cli").join("tmp");
    if !tmp.is_dir() {
        return vec![];
    }
    let mut out = Vec::new();
    let Ok(projects) = fs::read_dir(&tmp) else {
        return out;
    };
    for project in projects.flatten().filter(|e| e.path().is_dir()) {
        let chats = project.path().join("chats");
        if let Ok(entries) = fs::read_dir(&chats) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() && is_gemini_session_file(&path) {
                    if let Ok(session) = jsonl_session(ChatProvider::Antigravity, &tmp, &path) {
                        if session.turn_count > 0 {
                            out.push(session);
                        }
                    }
                } else if path.is_dir() {
                    let files = jsonl_files_in(&path, 1);
                    if !files.is_empty() {
                        if let Ok(session) =
                            jsonl_dir_session(ChatProvider::Antigravity, &tmp, &path, files)
                        {
                            if session.turn_count > 0 {
                                out.push(session);
                            }
                        }
                    }
                }
            }
        }
        if let Ok(entries) = fs::read_dir(project.path()) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() && is_gemini_checkpoint_file(&path) {
                    if let Ok(session) = jsonl_session(ChatProvider::Antigravity, &tmp, &path) {
                        if session.turn_count > 0 {
                            out.push(session);
                        }
                    }
                }
            }
        }
    }
    out
}

fn scan_kiro(workspace: &Path) -> Vec<ChatSession> {
    let _ = workspace;
    let Some(home) = dirs::home_dir() else {
        return vec![];
    };
    let root = env::var("KIRO_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home.join(".kiro"))
        .join("sessions")
        .join("cli");
    if !root.is_dir() {
        return vec![];
    }
    let Ok(entries) = fs::read_dir(root) else {
        return vec![];
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .filter_map(|p| kiro_session(&p).ok())
        .collect()
}

fn scan_imported() -> Vec<ChatSession> {
    let dir = imports_dir();
    if !dir.is_dir() {
        return vec![];
    }
    WalkDir::new(dir)
        .max_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().to_string_lossy().ends_with(ARCHIVE_EXT))
        .filter_map(|e| imported_session(e.path()).ok())
        .collect()
}

fn imported_session(path: &Path) -> Result<ChatSession> {
    let archive: ChatArchive = serde_json::from_str(&fs::read_to_string(path)?)?;
    validate_archive(&archive)?;
    let meta = fs::metadata(path)?;
    Ok(ChatSession {
        id: archive.source_session_id.clone(),
        title: format!("{} (imported)", archive.title),
        provider: archive.source_provider,
        project_path: archive.project_path,
        created_at: archive.created_at,
        updated_at: archive
            .updated_at
            .unwrap_or_else(|| file_time_label(&meta, true)),
        source_path: Some(path.to_path_buf()),
        source_kind: ChatSourceKind::ImportedArchive,
        turn_count: archive.messages.len(),
        size_bytes: meta.len(),
        imported: true,
        trash_manifest: None,
    })
}

fn kiro_session(path: &Path) -> Result<ChatSession> {
    let meta: Value = serde_json::from_str(&fs::read_to_string(path)?)?;
    let file_meta = fs::metadata(path)?;
    let jsonl = path.with_extension("jsonl");
    let jsonl_meta = fs::metadata(&jsonl).ok();
    let id = str_field(&meta, &["session_id"])
        .map(ToOwned::to_owned)
        .or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| "kiro-session".into());
    let title = str_field(&meta, &["title"])
        .filter(|s| !s.trim().is_empty())
        .map(short_title)
        .or_else(|| first_kiro_prompt(&jsonl).map(|s| short_title(&s)))
        .unwrap_or_else(|| "Untitled Kiro chat".into());
    Ok(ChatSession {
        id,
        title,
        provider: ChatProvider::Kiro,
        project_path: str_field(&meta, &["cwd"]).unwrap_or("Kiro CLI").to_string(),
        created_at: str_field(&meta, &["created_at"]).map(ToOwned::to_owned),
        updated_at: str_field(&meta, &["updated_at"])
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| file_time_label(&file_meta, true)),
        source_path: Some(path.to_path_buf()),
        source_kind: ChatSourceKind::KiroCli,
        turn_count: kiro_turn_count(&jsonl),
        size_bytes: file_meta.len() + jsonl_meta.map(|m| m.len()).unwrap_or_default(),
        imported: false,
        trash_manifest: None,
    })
}

fn trashed_session(manifest_path: &Path) -> Result<ChatSession> {
    let manifest: DeleteManifest = serde_json::from_str(&fs::read_to_string(manifest_path)?)?;
    let source_path = manifest.trashed_path.clone();
    let source_kind = manifest.source_kind.unwrap_or_else(|| {
        if source_path.as_ref().is_some_and(|p| p.is_dir()) {
            ChatSourceKind::JsonlDir
        } else {
            ChatSourceKind::Jsonl
        }
    });
    let meta = source_path.as_ref().and_then(|p| fs::metadata(p).ok());
    let mut session = ChatSession {
        id: manifest.session_id.clone(),
        title: manifest
            .title
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| trash_title_fallback(&manifest)),
        provider: manifest.provider,
        project_path: manifest.project_path.clone().unwrap_or_else(|| {
            manifest
                .original_path
                .as_ref()
                .and_then(|p| p.parent())
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "AgentSwitch Trash".into())
        }),
        created_at: manifest.created_at.clone(),
        updated_at: manifest
            .updated_at
            .clone()
            .unwrap_or_else(|| format!("unix:{}", manifest.deleted_at_unix)),
        source_path,
        source_kind,
        turn_count: manifest.turn_count.unwrap_or_default(),
        size_bytes: manifest
            .size_bytes
            .or_else(|| meta.as_ref().map(fs::Metadata::len))
            .unwrap_or_default(),
        imported: manifest.imported.unwrap_or(false),
        trash_manifest: Some(manifest_path.to_path_buf()),
    };
    if session.source_path.is_some() {
        if let Ok(archive) = load_archive(&session) {
            session.title = archive.title;
            session.project_path = archive.project_path;
            session.created_at = archive.created_at;
            session.updated_at = archive
                .updated_at
                .unwrap_or_else(|| format!("unix:{}", manifest.deleted_at_unix));
            session.turn_count = archive.messages.len();
            session.imported = source_kind == ChatSourceKind::ImportedArchive;
        }
    }
    Ok(session)
}

fn trash_title_fallback(manifest: &DeleteManifest) -> String {
    manifest
        .trashed_path
        .as_ref()
        .and_then(|p| p.file_stem())
        .and_then(|s| s.to_str())
        .map(|s| {
            s.trim_end_matches(".delete")
                .replace('-', " ")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| manifest.session_id.clone())
}

fn scan_jsonl_root(provider: ChatProvider, root: &Path, max_depth: usize) -> Vec<ChatSession> {
    if !root.is_dir() {
        return vec![];
    }
    WalkDir::new(root)
        .max_depth(max_depth)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().extension().and_then(|e| e.to_str()) == Some("jsonl"))
        .filter_map(|e| jsonl_session(provider, root, e.path()).ok())
        .collect()
}

fn jsonl_session(provider: ChatProvider, root: &Path, path: &Path) -> Result<ChatSession> {
    let meta = fs::metadata(path)?;
    let parsed = parse_jsonl_meta(provider, path, root)?;
    let fallback_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("chat")
        .to_string();
    let title = parsed
        .title
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| format!("Untitled {} chat", provider.label()));
    Ok(ChatSession {
        id: parsed.id.unwrap_or(fallback_id),
        title,
        provider,
        project_path: parsed
            .project_path
            .unwrap_or_else(|| project_label_from_path(provider, root, path)),
        created_at: parsed.created_at,
        updated_at: parsed
            .updated_at
            .unwrap_or_else(|| file_time_label(&meta, true)),
        source_path: Some(path.to_path_buf()),
        source_kind: ChatSourceKind::Jsonl,
        turn_count: parsed.turn_count,
        size_bytes: meta.len(),
        imported: false,
        trash_manifest: None,
    })
}

fn parse_jsonl_meta(provider: ChatProvider, path: &Path, root: &Path) -> Result<SessionMeta> {
    let file = File::open(path)?;
    let mut meta = SessionMeta::default();
    for line in BufReader::new(file).lines().map_while(|r| r.ok()) {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        update_meta_from_event(provider, &value, &mut meta);
        if meta.project_path.is_none() && provider == ChatProvider::Claude {
            meta.project_path = Some(project_label_from_path(provider, root, path));
        }
    }
    Ok(meta)
}

fn jsonl_dir_session(
    provider: ChatProvider,
    root: &Path,
    dir: &Path,
    files: Vec<PathBuf>,
) -> Result<ChatSession> {
    let mut parsed = SessionMeta::default();
    let mut size = 0;
    let mut updated = None;
    for file in &files {
        if let Ok(meta) = fs::metadata(file) {
            size += meta.len();
            updated = Some(file_time_label(&meta, true));
        }
        merge_meta(&mut parsed, parse_jsonl_meta(provider, file, root)?);
    }
    let fallback_id = dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("chat")
        .to_string();
    Ok(ChatSession {
        title: parsed
            .title
            .clone()
            .unwrap_or_else(|| format!("Untitled {} chat", provider.label())),
        id: parsed.id.unwrap_or(fallback_id),
        provider,
        project_path: parsed
            .project_path
            .unwrap_or_else(|| project_label_from_path(provider, root, dir)),
        created_at: parsed.created_at,
        updated_at: parsed
            .updated_at
            .or(updated)
            .unwrap_or_else(|| "unknown".into()),
        source_path: Some(dir.to_path_buf()),
        source_kind: ChatSourceKind::JsonlDir,
        turn_count: parsed.turn_count,
        size_bytes: size,
        imported: false,
        trash_manifest: None,
    })
}

fn load_jsonl_archive(session: &ChatSession) -> Result<ChatArchive> {
    let source = session
        .source_path
        .as_ref()
        .ok_or_else(|| anyhow!("chat has no file path"))?;
    let mut messages = Vec::new();
    let mut tools = Vec::new();
    let mut raw_events = Vec::new();
    for path in jsonl_sources(source) {
        let file = File::open(path)?;
        for line in BufReader::new(file).lines().map_while(|r| r.ok()) {
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if let Some(message) = message_from_event(&value) {
                messages.push(message);
            }
            if let Some(tool) = tool_from_event(&value) {
                tools.push(tool);
            }
            raw_events.push(value);
        }
    }
    Ok(ChatArchive {
        schema_version: ARCHIVE_VERSION,
        source_provider: session.provider,
        source_session_id: session.id.clone(),
        title: session.title.clone(),
        project_path: session.project_path.clone(),
        created_at: session.created_at.clone(),
        updated_at: Some(session.updated_at.clone()),
        messages,
        tool_calls: tools,
        raw_events,
    })
}

fn load_kiro_archive(session: &ChatSession) -> Result<ChatArchive> {
    let source = session
        .source_path
        .as_ref()
        .ok_or_else(|| anyhow!("Kiro chat has no file path"))?;
    let meta_path = kiro_meta_path(source)?;
    let jsonl = meta_path.with_extension("jsonl");
    let mut messages = Vec::new();
    let mut tools = Vec::new();
    let mut raw_events = Vec::new();
    if let Ok(file) = File::open(jsonl) {
        for line in BufReader::new(file).lines().map_while(|r| r.ok()) {
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if let Some(message) = kiro_message_from_event(&value) {
                messages.push(message);
            }
            if let Some(tool) = kiro_tool_from_event(&value) {
                tools.push(tool);
            }
            raw_events.push(value);
        }
    }
    Ok(ChatArchive {
        schema_version: ARCHIVE_VERSION,
        source_provider: session.provider,
        source_session_id: session.id.clone(),
        title: session.title.clone(),
        project_path: session.project_path.clone(),
        created_at: session.created_at.clone(),
        updated_at: Some(session.updated_at.clone()),
        messages,
        tool_calls: tools,
        raw_events,
    })
}

fn update_meta_from_event(provider: ChatProvider, value: &Value, meta: &mut SessionMeta) {
    let event = value.get("payload").unwrap_or(value);
    if meta.id.is_none() {
        meta.id = str_field(event, &["id", "sessionId", "session_id"])
            .or_else(|| str_field(value, &["sessionId", "session_id", "uuid"]))
            .map(ToOwned::to_owned);
    }
    if meta.project_path.is_none() {
        meta.project_path = str_field(event, &["cwd", "project_path", "projectPath"])
            .or_else(|| first_string(event.get("directories")))
            .map(ToOwned::to_owned);
    }
    if meta.created_at.is_none() {
        meta.created_at = str_field(event, &["started_at", "startTime", "created_at"])
            .or_else(|| str_field(value, &["timestamp"]))
            .map(ToOwned::to_owned);
    }
    if let Some(ts) = str_field(event, &["lastUpdated", "updated_at"])
        .or_else(|| str_field(value, &["timestamp"]))
    {
        meta.updated_at = Some(ts.to_owned());
    }
    if meta.title.is_none() {
        meta.title = str_field(
            event,
            &["customTitle", "thread_name", "title", "summary", "name"],
        )
        .or_else(|| value.get("$set").and_then(|v| str_field(v, &["summary"])))
        .or_else(|| tool_title(event))
        .map(short_title);
    }
    if is_message_event(value) {
        meta.turn_count += 1;
    }
    if meta.project_path.is_none() && provider == ChatProvider::Gemini {
        meta.project_path =
            str_field(value, &["projectHash"]).map(|s| format!("Gemini project {s}"));
    }
    if meta.project_path.is_none() && provider == ChatProvider::Antigravity {
        meta.project_path =
            str_field(value, &["projectHash"]).map(|s| format!("Antigravity project {s}"));
    }
}

fn tool_title(value: &Value) -> Option<&str> {
    value
        .get("toolCalls")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(|v| v.get("args"))
        .and_then(|v| str_field(v, &["title", "summary"]))
}

fn is_message_event(value: &Value) -> bool {
    let event = value.get("payload").unwrap_or(value);
    str_field(event, &["role"]).is_some()
        || matches!(
            str_field(event, &["type"]).or_else(|| str_field(value, &["type"])),
            Some("user_message" | "assistant_message" | "user" | "assistant" | "tool_result")
        )
}

fn message_from_event(value: &Value) -> Option<ChatMessage> {
    let event = value.get("payload").unwrap_or(value);
    let payload_type = str_field(event, &["type"]).or_else(|| str_field(value, &["type"]));
    let role = str_field(event, &["role"]).or(match payload_type {
        Some("user_message") | Some("user") => Some("user"),
        Some("assistant_message") | Some("assistant") => Some("assistant"),
        Some("tool_result") => Some("tool"),
        _ => None,
    })?;
    let text = str_field(event, &["message", "content", "text"])
        .map(ToOwned::to_owned)
        .or_else(|| event.get("message").and_then(text_from_value))
        .or_else(|| event.get("content").and_then(text_from_value))
        .or_else(|| value.get("content").and_then(text_from_value))
        .unwrap_or_default();
    if text.trim().is_empty() {
        return None;
    }
    Some(ChatMessage {
        role: normalize_role(role).into(),
        timestamp: str_field(event, &["timestamp"])
            .or_else(|| str_field(value, &["timestamp"]))
            .map(ToOwned::to_owned),
        text,
    })
}

fn kiro_message_from_event(value: &Value) -> Option<ChatMessage> {
    let kind = str_field(value, &["kind"])?;
    let role = match kind {
        "Prompt" => "user",
        "AssistantMessage" => "assistant",
        "ToolResults" => "tool",
        _ => return None,
    };
    let data = value.get("data").unwrap_or(value);
    let text = data
        .get("content")
        .and_then(text_from_value)
        .unwrap_or_default();
    if text.trim().is_empty() {
        return None;
    }
    Some(ChatMessage {
        role: role.into(),
        timestamp: str_field(data, &["timestamp"]).map(ToOwned::to_owned),
        text,
    })
}

fn tool_from_event(value: &Value) -> Option<ChatToolCall> {
    let event = value.get("payload").unwrap_or(value);
    let tool = event
        .get("tool_call")
        .or_else(|| event.get("toolCall"))
        .or_else(|| event.get("toolCalls"))
        .or_else(|| event.get("tool_use"))?;
    let name = str_field(tool, &["name", "tool", "command"])
        .unwrap_or("tool")
        .to_string();
    Some(ChatToolCall {
        name,
        timestamp: str_field(event, &["timestamp"])
            .or_else(|| str_field(value, &["timestamp"]))
            .map(ToOwned::to_owned),
        summary: summarize_tool(tool),
    })
}

fn kiro_tool_from_event(value: &Value) -> Option<ChatToolCall> {
    if str_field(value, &["kind"]) != Some("ToolResults") {
        return None;
    }
    let data = value.get("data")?;
    Some(ChatToolCall {
        name: "tool".into(),
        timestamp: str_field(data, &["timestamp"]).map(ToOwned::to_owned),
        summary: data
            .get("results")
            .map(summarize_tool)
            .unwrap_or_else(|| "tool results".into()),
    })
}

fn validate_archive(archive: &ChatArchive) -> Result<()> {
    if archive.schema_version != ARCHIVE_VERSION {
        return Err(anyhow!(
            "unsupported AgentSwitch chat archive version {}",
            archive.schema_version
        ));
    }
    if archive.source_session_id.trim().is_empty() {
        return Err(anyhow!("chat archive is missing a source session id"));
    }
    Ok(())
}

fn imports_dir() -> PathBuf {
    data_dir().join("chats").join("imports")
}

fn trash_dir() -> PathBuf {
    data_dir().join("chats").join("trash")
}

fn data_dir() -> PathBuf {
    if let Ok(path) = env::var("AGENT_SWITCH_DATA_DIR") {
        return PathBuf::from(path);
    }
    dirs::data_local_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("AgentSwitch")
}

fn move_path(source: &Path, target: &Path) -> Result<()> {
    match fs::rename(source, target) {
        Ok(()) => Ok(()),
        Err(_) => {
            if source.is_dir() {
                copy_dir(source, target)?;
                fs::remove_dir_all(source)?;
            } else {
                fs::copy(source, target)?;
                fs::remove_file(source)?;
            }
            Ok(())
        }
    }
}

fn copy_dir(source: &Path, target: &Path) -> Result<()> {
    for entry in WalkDir::new(source).into_iter().filter_map(|e| e.ok()) {
        let rel = entry.path().strip_prefix(source)?;
        let dest = target.join(rel);
        if entry.file_type().is_dir() {
            fs::create_dir_all(dest)?;
        } else {
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(entry.path(), dest)?;
        }
    }
    Ok(())
}

fn available_restore_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("chat");
    let ext = path.extension().and_then(|s| s.to_str());
    for n in 2usize.. {
        let name = match ext {
            Some(ext) => format!("{stem}-restored-{n}.{ext}"),
            None => format!("{stem}-restored-{n}"),
        };
        let candidate = parent.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    path.to_path_buf()
}

fn project_label_from_path(provider: ChatProvider, root: &Path, path: &Path) -> String {
    match provider {
        ChatProvider::Claude => path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .map(|s| s.replace('-', std::path::MAIN_SEPARATOR_STR))
            .unwrap_or_else(|| "Claude project".into()),
        ChatProvider::Gemini => path
            .strip_prefix(root)
            .ok()
            .and_then(|p| p.components().next())
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .unwrap_or_else(|| "Gemini project".into()),
        ChatProvider::Antigravity => path
            .strip_prefix(root)
            .ok()
            .and_then(|p| p.components().next())
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .unwrap_or_else(|| "Antigravity project".into()),
        _ => path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| provider.label().into()),
    }
}

fn str_field<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
}

fn first_string(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_array)
        .and_then(|arr| arr.iter().find_map(Value::as_str))
}

fn kiro_meta_path(path: &Path) -> Result<PathBuf> {
    if path.is_dir() {
        fs::read_dir(path)?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
            .ok_or_else(|| anyhow!("Kiro trash entry has no metadata file"))
    } else {
        Ok(path.to_path_buf())
    }
}

fn kiro_turn_count(path: &Path) -> usize {
    let Ok(file) = File::open(path) else {
        return 0;
    };
    BufReader::new(file)
        .lines()
        .map_while(|r| r.ok())
        .filter_map(|line| serde_json::from_str::<Value>(&line).ok())
        .filter(|value| {
            matches!(
                str_field(value, &["kind"]),
                Some("Prompt" | "AssistantMessage")
            )
        })
        .count()
}

fn first_kiro_prompt(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    BufReader::new(file)
        .lines()
        .map_while(|r| r.ok())
        .filter_map(|line| serde_json::from_str::<Value>(&line).ok())
        .find_map(|value| {
            if str_field(&value, &["kind"]) == Some("Prompt") {
                value
                    .get("data")
                    .and_then(|v| v.get("content"))
                    .and_then(text_from_value)
            } else {
                None
            }
        })
}

fn text_from_value(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Array(items) => {
            let mut parts = Vec::new();
            for item in items {
                if let Some(text) = item
                    .get("text")
                    .and_then(Value::as_str)
                    .or_else(|| item.get("content").and_then(Value::as_str))
                    .or_else(|| item.get("data").and_then(Value::as_str))
                    .or_else(|| item.as_str())
                {
                    parts.push(text.to_string());
                } else if let Some(text) = item.get("data").and_then(text_from_value) {
                    parts.push(text);
                }
            }
            Some(parts.join("\n")).filter(|s| !s.trim().is_empty())
        }
        Value::Object(map) => map
            .get("text")
            .or_else(|| map.get("content"))
            .or_else(|| map.get("data"))
            .and_then(text_from_value),
        _ => None,
    }
}

fn codex_titles() -> HashMap<String, String> {
    let Some(home) = dirs::home_dir() else {
        return HashMap::new();
    };
    let path = env::var("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home.join(".codex"))
        .join("session_index.jsonl");
    let Ok(file) = File::open(path) else {
        return HashMap::new();
    };
    BufReader::new(file)
        .lines()
        .map_while(|r| r.ok())
        .filter_map(|line| serde_json::from_str::<Value>(&line).ok())
        .filter_map(|v| {
            Some((
                str_field(&v, &["id"])?.to_string(),
                str_field(&v, &["thread_name", "title"])?.to_string(),
            ))
        })
        .collect()
}

fn merge_meta(to: &mut SessionMeta, from: SessionMeta) {
    to.id = to.id.take().or(from.id);
    to.title = to.title.take().or(from.title);
    to.project_path = to.project_path.take().or(from.project_path);
    to.created_at = to.created_at.take().or(from.created_at);
    to.updated_at = from.updated_at.or(to.updated_at.take());
    to.turn_count += from.turn_count;
}

fn jsonl_sources(path: &Path) -> Vec<PathBuf> {
    if path.is_dir() {
        jsonl_files_in(path, 1)
    } else {
        vec![path.to_path_buf()]
    }
}

fn jsonl_files_in(dir: &Path, depth: usize) -> Vec<PathBuf> {
    let mut files: Vec<_> = WalkDir::new(dir)
        .max_depth(depth)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("jsonl"))
        .collect();
    files.sort();
    files
}

fn is_gemini_session_file(path: &Path) -> bool {
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    name.starts_with("session-")
        && matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("json" | "jsonl")
        )
}

fn is_gemini_checkpoint_file(path: &Path) -> bool {
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    name.starts_with("checkpoint-") && path.extension().and_then(|e| e.to_str()) == Some("json")
}

fn normalize_role(role: &str) -> &'static str {
    match role {
        "user_message" | "user" => "user",
        "assistant_message" | "assistant" | "model" => "assistant",
        "tool_result" | "tool" => "tool",
        "system" => "system",
        _ => "event",
    }
}

fn summarize_tool(value: &Value) -> String {
    let mut keys = HashMap::new();
    if let Some(obj) = value.as_object() {
        for (k, v) in obj {
            keys.insert(k.clone(), value_kind(v));
        }
    }
    serde_json::to_string(&keys).unwrap_or_else(|_| "tool call".into())
}

fn value_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn short_title(text: &str) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut title: String = one_line.chars().take(72).collect();
    if one_line.chars().count() > 72 {
        title.push_str("...");
    }
    title
}

fn safe_file_stem(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else if c.is_whitespace() || c == '.' {
                '-'
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('-').trim_matches('_');
    if trimmed.is_empty() {
        "chat".into()
    } else {
        trimmed.chars().take(80).collect()
    }
}

fn file_time_label(meta: &fs::Metadata, modified: bool) -> String {
    let time = if modified {
        meta.modified().ok()
    } else {
        meta.created().ok()
    }
    .unwrap_or_else(SystemTime::now);
    match time.duration_since(UNIX_EPOCH) {
        Ok(d) => format!("unix:{}", d.as_secs()),
        Err(_) => "unknown".into(),
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn parses_codex_jsonl_metadata_and_messages() {
        let dir = temp_test_dir("codex-jsonl");
        let file = dir.join("rollout-abc.jsonl");
        fs::write(
            &file,
            r#"{"timestamp":"2026-05-01T00:00:00Z","type":"session_meta","payload":{"id":"s1","cwd":"D:/work/app","started_at":"2026-05-01T00:00:00Z"}}"#
                .to_string()
                + "\n"
                + r#"{"timestamp":"2026-05-01T00:01:00Z","type":"event_msg","payload":{"type":"user_message","message":"Build chat manager please"}}"#
                + "\n"
                + r#"{"timestamp":"2026-05-01T00:02:00Z","type":"event_msg","payload":{"type":"assistant_message","message":"Done"}}"#,
        )
        .unwrap();
        let session = jsonl_session(ChatProvider::Codex, &dir, &file).unwrap();
        assert_eq!(session.id, "s1");
        assert_eq!(session.project_path, "D:/work/app");
        assert_eq!(session.turn_count, 2);
        assert_eq!(session.title, "Untitled Codex CLI chat");
        let archive = load_archive(&session).unwrap();
        assert_eq!(archive.messages.len(), 2);
    }

    #[test]
    fn imports_archive_after_validation() {
        let archive = ChatArchive {
            schema_version: ARCHIVE_VERSION,
            source_provider: ChatProvider::Claude,
            source_session_id: "abc".into(),
            title: "A useful chat".into(),
            project_path: "D:/work".into(),
            created_at: None,
            updated_at: None,
            messages: vec![ChatMessage {
                role: "user".into(),
                timestamp: None,
                text: "hello".into(),
            }],
            tool_calls: vec![],
            raw_events: vec![],
        };
        validate_archive(&archive).unwrap();
    }

    #[test]
    fn gemini_session_filter_skips_internal_chunks() {
        assert!(is_gemini_session_file(Path::new("session-abc.jsonl")));
        assert!(is_gemini_session_file(Path::new("session-abc.json")));
        assert!(!is_gemini_session_file(Path::new("0mhqht.jsonl")));
        assert!(is_gemini_checkpoint_file(Path::new("checkpoint-save.json")));
    }

    #[test]
    fn metadata_search_matches_known_fields() {
        let dir = temp_test_dir("metadata-search");
        let file = write_sample_jsonl(&dir, "search-a", "D:/work/search-app");
        let session = jsonl_session(ChatProvider::Codex, &dir, &file).unwrap();
        assert!(metadata_matches(&session, "codex local search-app"));
        assert!(metadata_matches(&session, "D:/work"));
        assert!(!metadata_matches(&session, "claude imported"));
    }

    #[test]
    fn exports_single_and_multi_zip_manifest() {
        let dir = temp_test_dir("export-zip");
        let file_a = write_sample_jsonl(&dir, "zip-a", "D:/work/a");
        let file_b = write_sample_jsonl(&dir, "zip-b", "D:/work/b");
        let a = jsonl_session(ChatProvider::Codex, &dir, &file_a).unwrap();
        let b = jsonl_session(ChatProvider::Codex, &dir, &file_b).unwrap();
        let single = dir.join("one.agentswitch-chat.json");
        export_session(&a, &single).unwrap();
        let archive: ChatArchive =
            serde_json::from_str(&fs::read_to_string(single).unwrap()).unwrap();
        assert_eq!(archive.source_session_id, "zip-a");
        let zip_path = dir.join("many.zip");
        let report = export_sessions_zip(&[a, b], &zip_path).unwrap();
        assert_eq!(report.ok, 2);
        assert_eq!(report.failed, 0);
        let file = File::open(zip_path).unwrap();
        let mut zip = zip::ZipArchive::new(file).unwrap();
        let mut manifest = String::new();
        zip.by_name("manifest.json")
            .unwrap()
            .read_to_string(&mut manifest)
            .unwrap();
        let manifest: ChatZipManifest = serde_json::from_str(&manifest).unwrap();
        assert_eq!(manifest.entries.len(), 2);
    }

    #[test]
    fn trash_scan_restore_and_delete_forever() {
        let dir = temp_test_dir("trash-flow");
        env::set_var("AGENT_SWITCH_DATA_DIR", dir.join("data"));
        let file = write_sample_jsonl(&dir, "trash-a", "D:/work/trash");
        let session = jsonl_session(ChatProvider::Codex, &dir, &file).unwrap();
        soft_delete(&session, &dir).unwrap();
        assert!(!file.exists());
        let trash = scan_trash();
        assert_eq!(trash.len(), 1);
        assert_eq!(trash[0].title, session.title);
        assert_eq!(trash[0].project_path, session.project_path);
        let restored = restore_from_trash(&trash[0]).unwrap();
        assert_eq!(restored, file);
        assert!(file.exists());
        let session = jsonl_session(ChatProvider::Codex, &dir, &file).unwrap();
        soft_delete(&session, &dir).unwrap();
        let trash = scan_trash();
        delete_trash_forever(&trash[0]).unwrap();
        assert!(scan_trash().is_empty());
        env::remove_var("AGENT_SWITCH_DATA_DIR");
    }

    #[test]
    fn scans_and_exports_kiro_cli_sessions() {
        let dir = temp_test_dir("kiro-cli");
        let kiro_home = dir.join(".kiro");
        let cli = kiro_home.join("sessions").join("cli");
        fs::create_dir_all(&cli).unwrap();
        let id = "11111111-2222-3333-4444-555555555555";
        fs::write(
            cli.join(format!("{id}.json")),
            format!(
                r#"{{"session_id":"{id}","cwd":"D:\\AI","created_at":"2026-05-01T00:00:00Z","updated_at":"2026-05-01T00:01:00Z","title":"Kiro title","session_state":{{}}}}"#
            ),
        )
        .unwrap();
        fs::write(
            cli.join(format!("{id}.jsonl")),
            r#"{"version":1,"kind":"Prompt","data":{"message_id":"u1","content":[{"kind":"text","data":"hello kiro"}]}}"#
                .to_string()
                + "\n"
                + r#"{"version":1,"kind":"AssistantMessage","data":{"message_id":"a1","content":[{"kind":"text","data":"hello back"}]}}"#,
        )
        .unwrap();
        env::set_var("KIRO_HOME", &kiro_home);
        let sessions = scan_kiro(&dir);
        env::remove_var("KIRO_HOME");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].title, "Kiro title");
        assert_eq!(sessions[0].turn_count, 2);
        let archive = load_archive(&sessions[0]).unwrap();
        assert_eq!(archive.messages.len(), 2);
        assert_eq!(archive.messages[0].text, "hello kiro");
    }

    fn write_sample_jsonl(dir: &Path, id: &str, cwd: &str) -> PathBuf {
        let file = dir.join(format!("{id}.jsonl"));
        fs::write(
            &file,
            format!(
                "{{\"timestamp\":\"2026-05-01T00:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"{id}\",\"cwd\":\"{cwd}\",\"started_at\":\"2026-05-01T00:00:00Z\"}}}}\n{{\"timestamp\":\"2026-05-01T00:01:00Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\",\"message\":\"hello\"}}}}"
            ),
        )
        .unwrap();
        file
    }

    fn temp_test_dir(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("agentswitch-{name}-{}", unix_now()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
