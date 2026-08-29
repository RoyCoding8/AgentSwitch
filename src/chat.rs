mod model;

pub use model::*;

use crate::config_store::atomic_write;
use crate::types::str_field;
use anyhow::{anyhow, Context, Result};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    env,
    fs::{self, File},
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use walkdir::WalkDir;

fn jsonl_lines(file: File) -> impl Iterator<Item = String> {
    let mut reader = BufReader::new(file);
    let mut buf = Vec::new();
    std::iter::from_fn(move || {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) => None,
            Ok(_) => {
                while buf.last() == Some(&b'\n') || buf.last() == Some(&b'\r') {
                    buf.pop();
                }
                Some(String::from_utf8_lossy(&buf).into_owned())
            }
            Err(_) => None,
        }
    })
}
use zip::{read::ZipArchive, write::SimpleFileOptions, CompressionMethod, ZipWriter};

const ARCHIVE_VERSION: u32 = 1;
const ARCHIVE_EXT: &str = "agentswitch-chat.json";
const MAX_ZIP_ENTRIES: usize = 1_000;
const MAX_ZIP_ENTRY_BYTES: u64 = 50 * 1024 * 1024;
const MAX_ZIP_TOTAL_BYTES: u64 = 200 * 1024 * 1024;

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
    from_database: Option<bool>,
    deleted_at_unix: u64,
}

#[derive(Default, Clone)]
struct SessionMeta {
    id: Option<String>,
    title: Option<String>,
    project_path: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
    turn_count: usize,
    role_turns: usize,
    marker_turns: usize,
}

fn timestamp_sort_key(label: &str) -> i64 {
    if let Some(raw) = label.strip_prefix("unix:") {
        return raw.parse::<i64>().unwrap_or(i64::MIN);
    }
    parse_iso_seconds(label).unwrap_or(i64::MIN)
}

fn parse_iso_seconds(label: &str) -> Option<i64> {
    let bytes = label.as_bytes();
    if bytes.len() < 19
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || (bytes[10] != b'T' && bytes[10] != b' ')
    {
        return None;
    }
    let year: i64 = label.get(0..4)?.parse().ok()?;
    let month: i64 = label.get(5..7)?.parse().ok()?;
    let day: i64 = label.get(8..10)?.parse().ok()?;
    let hour: i64 = label.get(11..13)?.parse().ok()?;
    let minute: i64 = label.get(14..16)?.parse().ok()?;
    let second: i64 = label.get(17..19)?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + hour * 3600 + minute * 60 + second)
}

fn provider_matches(filter: Option<crate::types::ProviderId>, provider: ChatProvider) -> bool {
    use crate::types::ProviderId;
    filter.map_or(true, |prov| {
        matches!(
            (prov, provider),
            (ProviderId::Claude, ChatProvider::Claude)
                | (ProviderId::Codex, ChatProvider::Codex)
                | (ProviderId::Antigravity, ChatProvider::Antigravity)
                | (ProviderId::Kiro, ChatProvider::Kiro)
                | (ProviderId::OpenCode, ChatProvider::OpenCode)
                | (ProviderId::Zcode, ChatProvider::Zcode)
        )
    })
}

fn sort_sessions(sessions: &mut [ChatSession]) {
    sessions.sort_by(|a, b| {
        a.provider
            .cmp(&b.provider)
            .then(a.project_path.cmp(&b.project_path))
            .then(timestamp_sort_key(&b.updated_at).cmp(&timestamp_sort_key(&a.updated_at)))
            .then(a.title.cmp(&b.title))
    });
}

pub fn scan_all(provider_filter: Option<crate::types::ProviderId>) -> Vec<ChatSession> {
    let mut sessions = Vec::new();
    let include = |p: ChatProvider| provider_matches(provider_filter, p);
    if include(ChatProvider::Claude) {
        sessions.extend(scan_claude());
    }
    if include(ChatProvider::Codex) {
        sessions.extend(scan_codex());
    }
    if include(ChatProvider::Kiro) {
        sessions.extend(scan_kiro());
    }
    if include(ChatProvider::OpenCode) {
        sessions.extend(scan_opencode());
    }
    if include(ChatProvider::Zcode) {
        sessions.extend(scan_zcode());
    }
    sessions.extend(scan_imported().into_iter().filter(|s| include(s.provider)));
    sort_sessions(&mut sessions);
    sessions
}

pub fn scan_trash(provider_filter: Option<crate::types::ProviderId>) -> Vec<ChatSession> {
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
        .filter(|session| provider_matches(provider_filter, session.provider))
        .collect();
    sort_sessions(&mut sessions);
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
        ChatSourceKind::OpenCodeDb => load_opencode_archive(session),
    }
}

pub fn export_session(session: &ChatSession, target: &Path) -> Result<()> {
    let archive = load_archive(session)?;
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    atomic_write(target, serde_json::to_string_pretty(&archive)?.as_bytes())?;
    Ok(())
}

pub fn export_sessions_zip(sessions: &[ChatSession], target: &Path) -> Result<BatchReport> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let staging = target.with_extension("zip.part");
    let file = File::create(&staging)?;
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
    fs::rename(&staging, target)?;
    Ok(report)
}

pub fn import_archive(path: &Path, project_dir: Option<&Path>) -> Result<PathBuf> {
    let bytes = fs::read(path)?;
    if bytes.len() as u64 > MAX_ZIP_ENTRY_BYTES {
        anyhow::bail!("chat archive exceeds the size limit");
    }
    let archive: ChatArchive = serde_json::from_slice(&bytes)?;
    validate_archive(&archive)?;
    persist_imported_archive(archive, project_dir)
}

fn persist_imported_archive(
    mut archive: ChatArchive,
    project_dir: Option<&Path>,
) -> Result<PathBuf> {
    match archive.source_provider {
        ChatProvider::Kiro => return restore_kiro_native(&archive, project_dir),
        ChatProvider::Codex => return restore_codex_native(&archive, project_dir),
        ChatProvider::Claude => return restore_claude_native(&archive, project_dir),
        ChatProvider::OpenCode => {
            if let Some(p) = opencode_db_path() {
                return restore_opencode_native(&archive, project_dir, &p);
            }
        }
        ChatProvider::Zcode => {
            if let Some(p) = zcode_db_path() {
                return restore_zcode_native(&archive, project_dir, &p);
            }
        }
        _ => {}
    }
    if let Some(d) = project_dir {
        archive.project_path = d.to_string_lossy().to_string();
    }
    let dir = imports_dir();
    fs::create_dir_all(&dir)?;
    let base = safe_file_stem(&format!(
        "{}-{}",
        archive.source_provider.id(),
        archive.title
    ));
    let target = unique_path(&dir, &base, ARCHIVE_EXT);
    fs::write(&target, serde_json::to_string_pretty(&archive)?)?;
    Ok(target)
}

pub fn conversion_targets() -> Vec<ChatProvider> {
    ChatProvider::ALL
        .iter()
        .copied()
        .filter(|provider| *provider != ChatProvider::Antigravity)
        .collect()
}

pub fn convertible_targets(from: ChatProvider) -> Vec<ChatProvider> {
    conversion_targets()
        .into_iter()
        .filter(|provider| *provider != from)
        .collect()
}

pub fn convert_session(session: &ChatSession, target: ChatProvider) -> Result<PathBuf> {
    if target == ChatProvider::Antigravity || session.provider == ChatProvider::Antigravity {
        anyhow::bail!(
            "Antigravity chats are encrypted inside the CLI and cannot be read or written here"
        );
    }
    if session.provider == target {
        anyhow::bail!("chat is already a {} conversation", target.label());
    }
    let mut archive = load_archive(session)?;
    archive.raw_events.clear();
    write_converted(target, &archive)
}

fn write_converted(target: ChatProvider, archive: &ChatArchive) -> Result<PathBuf> {
    match target {
        ChatProvider::Claude => restore_claude_native(archive, None),
        ChatProvider::Codex => restore_codex_native(archive, None),
        ChatProvider::Kiro => restore_kiro_native(archive, None),
        ChatProvider::OpenCode => {
            let db =
                opencode_db_path().ok_or_else(|| anyhow!("no OpenCode chat database found"))?;
            restore_opencode_native(archive, None, &db)
        }
        ChatProvider::Zcode => {
            let db = zcode_db_path().ok_or_else(|| anyhow!("no ZCode chat database found"))?;
            restore_zcode_native(archive, None, &db)
        }
        ChatProvider::Antigravity => {
            anyhow::bail!("Antigravity chats are encrypted inside the CLI and cannot be written")
        }
    }
}

pub fn convert_archive_file(input: &Path, target: ChatProvider) -> Result<(PathBuf, usize)> {
    if target == ChatProvider::Antigravity {
        anyhow::bail!("Antigravity chats are encrypted inside the CLI and cannot be written");
    }
    let is_zip = input
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"));
    if is_zip {
        convert_archive_zip(input, target)
    } else {
        convert_archive_json(input, target).map(|path| (path, 0))
    }
}

fn retag_archive_for(archive: &mut ChatArchive, target: ChatProvider) -> Result<()> {
    if archive.source_provider == ChatProvider::Antigravity {
        anyhow::bail!("Antigravity chats are encrypted inside the CLI and cannot be converted");
    }
    archive.source_provider = target;
    archive.raw_events.clear();
    Ok(())
}

fn convert_archive_json(input: &Path, target: ChatProvider) -> Result<PathBuf> {
    let bytes = fs::read(input)?;
    if bytes.len() as u64 > MAX_ZIP_ENTRY_BYTES {
        anyhow::bail!("chat archive exceeds the size limit");
    }
    let mut archive: ChatArchive = serde_json::from_slice(&bytes)?;
    validate_archive(&archive)?;
    retag_archive_for(&mut archive, target)?;
    let dir = input.parent().unwrap_or(Path::new("."));
    let raw_name = input
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("chat");
    let stem = raw_name
        .strip_suffix(ARCHIVE_EXT)
        .map(|stem| stem.strip_suffix('.').unwrap_or(stem))
        .unwrap_or_else(|| raw_name);
    let base = safe_file_stem(&format!("{stem}-{}", target.id()));
    let out = unique_path(dir, &base, ARCHIVE_EXT);
    fs::write(&out, serde_json::to_string_pretty(&archive)?)?;
    Ok(out)
}

fn add_zip_bytes(total: &mut u64, size: u64) -> Result<()> {
    *total = total
        .checked_add(size)
        .ok_or_else(|| anyhow!("ZIP uncompressed size overflow"))?;
    if *total > MAX_ZIP_TOTAL_BYTES {
        anyhow::bail!("ZIP exceeds the total uncompressed size limit");
    }
    Ok(())
}

fn convert_archive_zip(input: &Path, target: ChatProvider) -> Result<(PathBuf, usize)> {
    let mut zip = ZipArchive::new(File::open(input)?)?;
    if zip.len() > MAX_ZIP_ENTRIES {
        anyhow::bail!("ZIP contains too many entries (maximum {MAX_ZIP_ENTRIES})");
    }
    let mut entries: Vec<(String, ChatArchive)> = Vec::new();
    let mut extras: Vec<(String, Vec<u8>)> = Vec::new();
    let mut skipped = 0usize;
    let mut total_bytes = 0u64;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let name = entry.name().to_string();
        if !name.ends_with(ARCHIVE_EXT) {
            if !name.ends_with('/') && name != "manifest.json" {
                add_zip_bytes(&mut total_bytes, entry.size())?;
                let mut buf = Vec::with_capacity(entry.size() as usize);
                entry.read_to_end(&mut buf)?;
                extras.push((name, buf));
            }
            continue;
        }
        if entry.size() > MAX_ZIP_ENTRY_BYTES {
            anyhow::bail!("ZIP entry '{name}' exceeds the uncompressed size limit");
        }
        add_zip_bytes(&mut total_bytes, entry.size())?;
        let mut buf = String::with_capacity(entry.size() as usize);
        entry.read_to_string(&mut buf)?;
        let mut archive: ChatArchive = serde_json::from_str(&buf)
            .map_err(|error| anyhow!("ZIP entry '{name}' is not a valid chat archive: {error}"))?;
        validate_archive(&archive)
            .map_err(|error| anyhow!("ZIP entry '{name}' is not a valid chat archive: {error}"))?;
        if retag_archive_for(&mut archive, target).is_err() {
            skipped += 1;
            continue;
        }
        entries.push((name, archive));
    }
    if entries.is_empty() {
        anyhow::bail!(
            "no convertible chats found in this ZIP{}",
            if skipped > 0 {
                format!(" ({skipped} Antigravity chat(s) cannot be converted)")
            } else {
                String::new()
            }
        );
    }
    let dir = input.parent().unwrap_or(Path::new("."));
    let stem = input
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("chats");
    let out_path = unique_path(
        dir,
        &format!("{}-{}", safe_file_stem(stem), target.id()),
        "zip",
    );
    let staging = out_path.with_extension("zip.part");
    let mut out = ZipWriter::new(File::create(&staging)?);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let mut manifest_entries = Vec::new();
    for (name, archive) in &entries {
        out.start_file(name.as_str(), options)?;
        out.write_all(serde_json::to_string_pretty(archive)?.as_bytes())?;
        manifest_entries.push(ChatZipManifestEntry {
            provider: target,
            session_id: archive.source_session_id.clone(),
            title: archive.title.clone(),
            project_path: archive.project_path.clone(),
            archive_path: name.clone(),
        });
    }
    for (name, bytes) in &extras {
        out.start_file(name.as_str(), options)?;
        out.write_all(bytes)?;
    }
    let manifest = ChatZipManifest {
        schema_version: ARCHIVE_VERSION,
        exported_at_unix: unix_now(),
        entries: manifest_entries,
    };
    out.start_file("manifest.json", options)?;
    out.write_all(serde_json::to_string_pretty(&manifest)?.as_bytes())?;
    out.finish()?;
    fs::rename(&staging, &out_path)?;
    Ok((out_path, skipped))
}

fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let suffix = if ext.is_empty() {
        String::new()
    } else {
        format!(".{ext}")
    };
    std::iter::once(dir.join(format!("{stem}{suffix}")))
        .chain((2u32..).map(|n| dir.join(format!("{stem}-{n}{suffix}"))))
        .find(|candidate| !candidate.exists())
        .expect("the candidate sequence is infinite")
}

pub fn import_zip(path: &Path, project_dir: Option<&Path>) -> Result<BatchReport> {
    let file = File::open(path)?;
    let mut zip = ZipArchive::new(file)?;
    if zip.len() > MAX_ZIP_ENTRIES {
        anyhow::bail!("ZIP contains too many entries (maximum {MAX_ZIP_ENTRIES})");
    }
    let mut report = BatchReport::default();
    let mut total_bytes = 0u64;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let name = entry.name().to_string();
        if !name.ends_with(ARCHIVE_EXT) {
            continue;
        }
        if entry.size() > MAX_ZIP_ENTRY_BYTES {
            anyhow::bail!("ZIP entry '{name}' exceeds the uncompressed size limit");
        }
        add_zip_bytes(&mut total_bytes, entry.size())?;
        let mut buf = String::with_capacity(entry.size() as usize);
        std::io::Read::read_to_string(&mut entry, &mut buf)?;
        let archive: ChatArchive = match serde_json::from_str(&buf) {
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
        match persist_imported_archive(archive, project_dir) {
            Ok(_) => report.ok += 1,
            Err(_) => report.failed += 1,
        }
    }
    Ok(report)
}

pub fn exports_dir() -> PathBuf {
    data_dir().join("chats").join("exports")
}

pub fn soft_delete(session: &ChatSession) -> Result<()> {
    if session.source_kind == ChatSourceKind::KiroCli {
        return soft_delete_kiro_session(session);
    }
    if session.source_kind == ChatSourceKind::OpenCodeDb {
        return soft_delete_db_session(session);
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
    let target = unique_path(&trash_dir, &stem, ext);
    move_path(source, &target)?;
    let manifest = DeleteManifest {
        original_path: Some(source.clone()),
        trashed_path: Some(target.clone()),
        ..base_manifest(session)
    };
    if let Err(error) = fs::write(
        target.with_extension("delete.json"),
        serde_json::to_string_pretty(&manifest)?,
    ) {
        let _ = fs::create_dir_all(
            source
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from(".")),
        );
        let _ = move_path(&target, source);
        return Err(error).context(
            "chat moved to trash but the manifest could not be written; it was moved back",
        );
    }
    if session.provider == ChatProvider::Codex {
        codex_state_unregister(source);
    }
    Ok(())
}

fn base_manifest(session: &ChatSession) -> DeleteManifest {
    DeleteManifest {
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
        original_path: None,
        trashed_path: None,
        source_kind: Some(session.source_kind),
        from_database: None,
        deleted_at_unix: unix_now(),
    }
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
    let target = unique_path(&trash_dir, &stem, "");
    fs::create_dir_all(&target)?;
    let base = meta_path.with_extension("");
    let manifest = DeleteManifest {
        original_path: Some(meta_path),
        trashed_path: Some(target.clone()),
        ..base_manifest(session)
    };
    let mut moved = Vec::new();
    let result = (|| -> Result<()> {
        for ext in ["json", "jsonl", "lock"] {
            let source_file = base.with_extension(ext);
            if source_file.exists() {
                let dest = target.join(
                    source_file
                        .file_name()
                        .ok_or_else(|| anyhow!("invalid Kiro session path"))?,
                );
                move_path(&source_file, &dest)?;
                moved.push((dest, source_file));
            }
        }
        Ok(())
    })()
    .and_then(|()| {
        fs::write(
            target.with_extension("delete.json"),
            serde_json::to_string_pretty(&manifest)?,
        )
        .map_err(Into::into)
    });
    if let Err(error) = result {
        for (dest, source_file) in moved {
            let _ = move_path(&dest, &source_file);
        }
        return Err(error).context("Kiro chat trash failed; session was rolled back");
    }
    Ok(())
}

fn soft_delete_db_session(session: &ChatSession) -> Result<()> {
    let db_path = session
        .source_path
        .as_ref()
        .ok_or_else(|| anyhow!("database chat has no database path"))?;
    let archive = load_archive(session)?;
    let trash_dir = trash_dir().join(session.provider.id());
    fs::create_dir_all(&trash_dir)?;
    let stem = safe_file_stem(&format!("{}-{}", session.id, session.title));
    let archive_path = unique_path(&trash_dir, &stem, ARCHIVE_EXT);
    atomic_write(
        &archive_path,
        serde_json::to_string_pretty(&archive)?.as_bytes(),
    )?;
    let manifest = DeleteManifest {
        original_path: Some(db_path.clone()),
        trashed_path: Some(archive_path.clone()),
        source_kind: Some(ChatSourceKind::ImportedArchive),
        from_database: Some(true),
        ..base_manifest(session)
    };
    let manifest_path = archive_path.with_extension("delete.json");
    fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?)?;
    if let Err(error) = delete_db_session_rows(db_path, &session.id) {
        let _ = fs::remove_file(&manifest_path);
        let _ = fs::remove_file(&archive_path);
        return Err(error)
            .context("chat rows could not be deleted from the database; nothing was trashed");
    }
    Ok(())
}

fn finish_tx(conn: &Connection, result: Result<()>) -> Result<()> {
    match result {
        Ok(()) => conn.execute_batch("COMMIT")?,
        Err(error) => {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(error);
        }
    }
    Ok(())
}

fn delete_db_session_rows(db_path: &Path, session_id: &str) -> Result<()> {
    let conn = Connection::open(db_path)?;
    conn.busy_timeout(std::time::Duration::from_secs(3))?;
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| -> Result<()> {
        let tables = sqlite_tables(&conn)?;
        if tables.contains("part") {
            conn.execute(
                "DELETE FROM part WHERE session_id = ?1",
                rusqlite::params![session_id],
            )?;
        }
        if tables.contains("message") {
            conn.execute(
                "DELETE FROM message WHERE session_id = ?1",
                rusqlite::params![session_id],
            )?;
        }
        if tables.contains("session_message") {
            conn.execute(
                "DELETE FROM session_message WHERE session_id = ?1",
                rusqlite::params![session_id],
            )?;
        }
        conn.execute(
            "DELETE FROM session WHERE id = ?1",
            rusqlite::params![session_id],
        )?;
        Ok(())
    })();
    finish_tx(&conn, result)
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
    if manifest.from_database == Some(true) {
        return restore_db_session_from_trash(&manifest, manifest_path);
    }
    if manifest.source_kind == Some(ChatSourceKind::KiroCli) && source.is_dir() {
        return restore_kiro_session(source, original, manifest_path);
    }
    let target = available_restore_path(original);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    move_path(source, &target)?;
    let registered = if manifest.provider == ChatProvider::Codex
        && manifest.source_kind != Some(ChatSourceKind::ImportedArchive)
        && !manifest.imported.unwrap_or(false)
    {
        codex_state_register(
            &target,
            &manifest.session_id,
            manifest.project_path.as_deref().unwrap_or(""),
            manifest.title.as_deref().unwrap_or(""),
            None,
            manifest.created_at.as_deref(),
        )
    } else {
        Ok(())
    };
    registered?;
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
    if manifest.provider == ChatProvider::Codex && manifest.from_database != Some(true) {
        if let Some(original) = manifest.original_path.as_deref() {
            codex_state_unregister(original);
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
    crate::provider::env_path("CLAUDE_CONFIG_DIR")
        .or_else(|| dirs::home_dir().map(|home| home.join(".claude")))
}

fn scan_codex() -> Vec<ChatSession> {
    let mut roots = Vec::new();
    if let Some(home) = codex_home() {
        roots.push(home.join("sessions"));
        roots.push(home.join("archived_sessions"));
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

fn opencode_db_path() -> Option<PathBuf> {
    if let Some(custom) = env::var_os("OPENCODE_DB").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(custom)).filter(|path| path.is_file());
    }
    dirs::data_local_dir()
        .map(|path| path.join("opencode").join("opencode.db"))
        .filter(|path| path.is_file())
}

fn zcode_db_path() -> Option<PathBuf> {
    if let Some(custom) = env::var_os("ZCODE_DB").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(custom)).filter(|path| path.is_file());
    }
    dirs::home_dir()
        .map(|home| home.join(".zcode").join("cli").join("db").join("db.sqlite"))
        .filter(|path| path.is_file())
}

fn scan_opencode() -> Vec<ChatSession> {
    let Some(db_path) = opencode_db_path() else {
        return vec![];
    };
    scan_sqlite_sessions(&db_path, ChatProvider::OpenCode)
}

fn scan_zcode() -> Vec<ChatSession> {
    let Some(db_path) = zcode_db_path() else {
        return vec![];
    };
    scan_sqlite_sessions(&db_path, ChatProvider::Zcode)
}

fn scan_sqlite_sessions(db_path: &Path, provider: ChatProvider) -> Vec<ChatSession> {
    let Ok(conn) = Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return vec![];
    };
    let current = conn.prepare(
        "SELECT s.id, s.title, s.directory, s.time_created, s.time_updated, \
         (SELECT COUNT(*) FROM message m WHERE m.session_id = s.id) as msg_count \
         FROM session s ORDER BY s.time_updated DESC",
    );
    let mut stmt = match current {
        Ok(stmt) => stmt,
        Err(_) => {
            let Ok(legacy) = conn.prepare(
                "SELECT s.id, s.title, s.directory, s.time_created, s.time_updated, \
                 (SELECT COUNT(*) FROM session_message m WHERE m.session_id = s.id) as msg_count \
                 FROM session s ORDER BY s.time_updated DESC",
            ) else {
                return vec![];
            };
            legacy
        }
    };
    let rows: Vec<(String, String, String, i64, i64, i64)> = stmt
        .query_map([], |row| {
            let id: String = row.get(0)?;
            let title: String = row.get(1)?;
            let directory: String = row.get(2)?;
            let time_created: i64 = row.get(3)?;
            let time_updated: i64 = row.get(4)?;
            let msg_count: i64 = row.get(5)?;
            Ok((id, title, directory, time_created, time_updated, msg_count))
        })
        .ok()
        .map(|r| r.filter_map(|r| r.ok()).collect())
        .unwrap_or_default();
    rows.into_iter()
        .map(
            |(id, title, directory, created, updated, msg_count)| ChatSession {
                id: id.clone(),
                title,
                provider,
                project_path: directory,
                created_at: Some(timestamp_label(created)),
                updated_at: timestamp_label(updated),
                source_path: Some(db_path.to_path_buf()),
                source_kind: ChatSourceKind::OpenCodeDb,
                turn_count: msg_count as usize,
                size_bytes: 0,
                imported: false,
                subagent: false,
                trash_manifest: None,
            },
        )
        .collect()
}

fn load_opencode_archive(session: &ChatSession) -> Result<ChatArchive> {
    let db_path = session
        .source_path
        .as_ref()
        .ok_or_else(|| anyhow!("OpenCode chat has no database path"))?;
    let conn = Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let tables = sqlite_tables(&conn)?;
    let mut messages = Vec::new();
    let mut tools = Vec::new();
    if tables.contains("message") && tables.contains("part") {
        load_opencode_current(&conn, &session.id, &mut messages, &mut tools)?;
    } else if tables.contains("session_message") {
        load_opencode_legacy(&conn, &session.id, &mut messages, &mut tools)?;
    } else {
        anyhow::bail!("unsupported chat database schema: no known message tables");
    }
    Ok(archive_from(session, messages, tools, Vec::new()))
}

fn archive_from(
    session: &ChatSession,
    messages: Vec<ChatMessage>,
    tool_calls: Vec<ChatToolCall>,
    raw_events: Vec<Value>,
) -> ChatArchive {
    ChatArchive {
        schema_version: ARCHIVE_VERSION,
        source_provider: session.provider,
        source_session_id: session.id.clone(),
        title: session.title.clone(),
        project_path: session.project_path.clone(),
        created_at: session.created_at.clone(),
        updated_at: Some(session.updated_at.clone()),
        messages,
        tool_calls,
        raw_events,
    }
}

fn sqlite_tables(conn: &Connection) -> Result<HashSet<String>> {
    let mut stmt = conn.prepare("SELECT name FROM sqlite_master WHERE type = 'table'")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let tables = rows.filter_map(|row| row.ok()).collect();
    Ok(tables)
}

fn sqlite_columns(conn: &Connection, table: &str) -> Result<HashSet<String>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    let columns = rows.filter_map(|row| row.ok()).collect();
    Ok(columns)
}

fn load_opencode_current(
    conn: &Connection,
    session_id: &str,
    messages: &mut Vec<ChatMessage>,
    tools: &mut Vec<ChatToolCall>,
) -> Result<()> {
    let mut stmt = conn.prepare(
        "SELECT m.id, m.data, p.data FROM message m LEFT JOIN part p ON p.message_id = m.id \
         WHERE m.session_id = ?1 ORDER BY m.time_created ASC, m.id ASC, p.id ASC",
    )?;
    let rows = stmt.query_map(rusqlite::params![session_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })?;
    let mut grouped: HashMap<String, (Value, Vec<Value>)> = HashMap::new();
    let mut order = Vec::new();
    for row in rows {
        let (message_id, message_data, part_data) = row?;
        let message: Value = serde_json::from_str(&message_data)?;
        if !grouped.contains_key(&message_id) {
            order.push(message_id.clone());
            grouped.insert(message_id.clone(), (message, Vec::new()));
        }
        if let Some(part_data) = part_data {
            if let Ok(part) = serde_json::from_str::<Value>(&part_data) {
                if let Some((_, parts)) = grouped.get_mut(&message_id) {
                    parts.push(part);
                }
            }
        }
    }
    for key in order {
        let Some((message, parts)) = grouped.remove(&key) else {
            continue;
        };
        let role = str_field(&message, &["role", "type"]).unwrap_or("assistant");
        let text = parts
            .iter()
            .map(extract_text_field)
            .filter(|text| !text.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        if !text.is_empty() {
            messages.push(ChatMessage {
                role: normalize_role(role).into(),
                timestamp: None,
                text,
            });
        }
        for part in parts {
            if let Some(tool_name) = str_field(&part, &["name", "tool"]) {
                tools.push(ChatToolCall {
                    name: tool_name.into(),
                    timestamp: None,
                    summary: summarize_tool(&part),
                });
            }
        }
    }
    Ok(())
}

fn load_opencode_legacy(
    conn: &Connection,
    session_id: &str,
    messages: &mut Vec<ChatMessage>,
    tools: &mut Vec<ChatToolCall>,
) -> Result<()> {
    let mut stmt = conn.prepare(
        "SELECT data FROM session_message WHERE session_id = ?1 ORDER BY time_created ASC",
    )?;
    for row in stmt.query_map(rusqlite::params![session_id], |row| row.get::<_, String>(0))? {
        let Ok(value) = serde_json::from_str::<Value>(&row?) else {
            continue;
        };
        if let Some(role) = str_field(&value, &["role"]) {
            let text = extract_text_field(&value);
            if !text.trim().is_empty() {
                messages.push(ChatMessage {
                    role: normalize_role(role).into(),
                    timestamp: None,
                    text,
                });
            }
        }
        if let Some(tool_name) = str_field(&value, &["name", "tool"]) {
            tools.push(ChatToolCall {
                name: tool_name.into(),
                timestamp: None,
                summary: summarize_tool(&value),
            });
        }
    }
    Ok(())
}

fn extract_text_field(val: &Value) -> String {
    str_field(val, &["content", "text", "message"])
        .map(ToOwned::to_owned)
        .or_else(|| {
            val.get("content").and_then(|c| {
                if let Some(s) = c.as_str() {
                    Some(s.to_string())
                } else if let Some(arr) = c.as_array() {
                    let parts: Vec<String> = arr
                        .iter()
                        .filter_map(|item| {
                            str_field(item, &["text"])
                                .or_else(|| str_field(item, &["content"]))
                                .map(ToOwned::to_owned)
                        })
                        .collect();
                    if parts.is_empty() {
                        None
                    } else {
                        Some(parts.join("\n"))
                    }
                } else {
                    None
                }
            })
        })
        .unwrap_or_default()
}

fn scan_kiro() -> Vec<ChatSession> {
    let Ok(root) = kiro_sessions_dir() else {
        return vec![];
    };
    if !root.is_dir() {
        return vec![];
    }
    let Ok(entries) = fs::read_dir(root) else {
        return vec![];
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|extension| extension.to_str()) == Some("json"))
        .filter_map(|path| kiro_session(&path).ok())
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
        subagent: false,
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
        subagent: str_field(&meta, &["session_created_reason"]) == Some("subagent"),
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
        subagent: false,
        trash_manifest: Some(manifest_path.to_path_buf()),
    };
    let manifest_has_meta = manifest
        .title
        .as_deref()
        .is_some_and(|title| !title.trim().is_empty());
    if !manifest_has_meta && session.source_path.is_some() {
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
        subagent: false,
        trash_manifest: None,
    })
}

fn parse_jsonl_meta(provider: ChatProvider, path: &Path, root: &Path) -> Result<SessionMeta> {
    let meta = fs::metadata(path)?;
    if let Some(cached) = cached_meta(provider, path, &meta) {
        return Ok(cached);
    }
    let file = File::open(path)?;
    let mut meta_parsed = SessionMeta::default();
    for line in jsonl_lines(file) {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        update_meta_from_event(provider, &value, &mut meta_parsed);
        if meta_parsed.project_path.is_none() && provider == ChatProvider::Claude {
            meta_parsed.project_path = Some(project_label_from_path(provider, root, path));
        }
    }
    store_cached_meta(provider, path, &meta, &meta_parsed);
    Ok(meta_parsed)
}

type MetaCache = HashMap<(ChatProvider, PathBuf), (std::time::Duration, u64, SessionMeta)>;

fn meta_cache() -> &'static std::sync::Mutex<MetaCache> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<MetaCache>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

fn cached_meta(provider: ChatProvider, path: &Path, meta: &fs::Metadata) -> Option<SessionMeta> {
    let current_mtime = modified_since_epoch(meta)?;
    let cache = meta_cache().lock().ok()?;
    cache
        .get(&(provider, path.to_path_buf()))
        .filter(|(mtime, len, _)| *len == meta.len() && *mtime == current_mtime)
        .map(|(_, _, parsed)| parsed.clone())
}

fn store_cached_meta(
    provider: ChatProvider,
    path: &Path,
    meta: &fs::Metadata,
    parsed: &SessionMeta,
) {
    let Some(current_mtime) = modified_since_epoch(meta) else {
        return;
    };
    let Ok(mut cache) = meta_cache().lock() else {
        return;
    };
    if cache.len() > 4_096 {
        cache.clear();
    }
    cache.insert(
        (provider, path.to_path_buf()),
        (current_mtime, meta.len(), parsed.clone()),
    );
}

fn modified_since_epoch(meta: &fs::Metadata) -> Option<std::time::Duration> {
    meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()
}

#[allow(dead_code)]
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
        subagent: false,
        trash_manifest: None,
    })
}

fn load_jsonl_archive(session: &ChatSession) -> Result<ChatArchive> {
    let source = session
        .source_path
        .as_ref()
        .ok_or_else(|| anyhow!("chat has no file path"))?;
    let mut messages: Vec<(ChatMessage, bool)> = Vec::new();
    let mut tools = Vec::new();
    let mut raw_events = Vec::new();
    for path in jsonl_sources(source) {
        let file = File::open(path)?;
        for line in jsonl_lines(file) {
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if let Some((message, from_marker)) = message_from_event(&value) {
                messages.push((message, from_marker));
            }
            if let Some(tool) = tool_from_event(&value) {
                tools.push(tool);
            }
            raw_events.push(value);
        }
    }
    let has_authoritative = messages.iter().any(|(_, marker)| !marker);
    let messages: Vec<ChatMessage> = messages
        .into_iter()
        .filter(|(message, marker)| {
            !(has_authoritative && *marker && matches!(message.role.as_str(), "user" | "assistant"))
        })
        .map(|(message, _)| message)
        .collect();
    Ok(archive_from(session, messages, tools, raw_events))
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
    let mut tool_ids: HashMap<String, usize> = HashMap::new();
    let mut raw_events = Vec::new();

    if let Ok(meta_str) = fs::read_to_string(&meta_path) {
        if let Ok(meta_val) = serde_json::from_str::<Value>(&meta_str) {
            if let Some(state) = meta_val.get("session_state") {
                raw_events
                    .push(serde_json::json!({"__agentswitch_kiro_meta": {"session_state": state}}));
            }
        }
    }

    if let Ok(file) = File::open(jsonl) {
        for line in jsonl_lines(file) {
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if let Some(message) = kiro_message_from_event(&value) {
                messages.push(message);
            }
            kiro_collect_tools(&value, &mut tool_ids, &mut tools);
            raw_events.push(value);
        }
    }
    Ok(archive_from(session, messages, tools, raw_events))
}

fn update_meta_from_event(_provider: ChatProvider, value: &Value, meta: &mut SessionMeta) {
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
    if let Some(role) = str_field(event, &["role"]) {
        if !matches!(role, "system" | "developer") {
            meta.role_turns += 1;
        }
    } else if matches!(
        str_field(event, &["type"]).or_else(|| str_field(value, &["type"])),
        Some(
            "user_message"
                | "assistant_message"
                | "agent_message"
                | "user"
                | "assistant"
                | "tool_result"
        )
    ) {
        meta.marker_turns += 1;
    }
    meta.turn_count = if meta.role_turns > 0 {
        meta.role_turns
    } else {
        meta.marker_turns
    };
}

fn tool_title(value: &Value) -> Option<&str> {
    value
        .get("toolCalls")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(|v| v.get("args"))
        .and_then(|v| str_field(v, &["title", "summary"]))
}

fn message_from_event(value: &Value) -> Option<(ChatMessage, bool)> {
    let event = value.get("payload").unwrap_or(value);
    let payload_type = str_field(event, &["type"]).or_else(|| str_field(value, &["type"]));
    let (role, from_marker) = match str_field(event, &["role"]) {
        Some("system" | "developer") => return None,
        Some(role) => (role, false),
        None => match payload_type? {
            "user_message" | "user" => ("user", true),
            "assistant_message" | "agent_message" | "assistant" => ("assistant", true),
            "tool_result" => ("tool", true),
            _ => return None,
        },
    };
    let text = str_field(event, &["message", "content", "text"])
        .map(ToOwned::to_owned)
        .or_else(|| event.get("message").and_then(text_from_value))
        .or_else(|| event.get("content").and_then(text_from_value))
        .or_else(|| value.get("content").and_then(text_from_value))
        .unwrap_or_default();
    if text.trim().is_empty() {
        return None;
    }
    Some((
        ChatMessage {
            role: normalize_role(role).into(),
            timestamp: str_field(event, &["timestamp"])
                .or_else(|| str_field(value, &["timestamp"]))
                .map(ToOwned::to_owned),
            text,
        },
        from_marker,
    ))
}

fn kiro_message_from_event(value: &Value) -> Option<ChatMessage> {
    let role = match str_field(value, &["kind"])? {
        "Prompt" => "user",
        "AssistantMessage" => "assistant",
        _ => return None,
    };
    let data = value.get("data").unwrap_or(value);
    let text = data
        .get("content")
        .map(kiro_text_from_content)
        .unwrap_or_default();
    if text.trim().is_empty() && kiro_tool_use_blocks(data).next().is_none() {
        return None;
    }
    Some(ChatMessage {
        role: role.into(),
        timestamp: kiro_event_timestamp(data),
        text,
    })
}

fn kiro_event_timestamp(data: &Value) -> Option<String> {
    let value = data
        .get("meta")
        .and_then(|meta| meta.get("timestamp"))
        .or_else(|| data.get("timestamp"))?;
    let secs = match value.as_i64() {
        Some(secs) => secs,
        None => parse_epoch_millis(value.as_str()?)? / 1000,
    };
    Some(fmt_iso(secs.max(0) as u64))
}

fn kiro_text_from_content(content: &Value) -> String {
    let Some(blocks) = content.as_array() else {
        return text_from_value(content).unwrap_or_default();
    };
    blocks
        .iter()
        .filter(|block| !matches!(str_field(block, &["kind"]), Some("toolUse" | "toolResult")))
        .filter_map(|block| block.get("data").and_then(text_from_value))
        .collect::<Vec<_>>()
        .join("\n")
}

fn kiro_tool_use_blocks(data: &Value) -> impl Iterator<Item = &Value> {
    data.get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|block| str_field(block, &["kind"]) == Some("toolUse"))
        .filter_map(|block| block.get("data"))
}

fn kiro_collect_tools(
    value: &Value,
    calls: &mut HashMap<String, usize>,
    tools: &mut Vec<ChatToolCall>,
) {
    let kind = str_field(value, &["kind"]);
    let data = value.get("data").unwrap_or(value);
    if kind == Some("AssistantMessage") {
        for block in kiro_tool_use_blocks(data) {
            if let Some(id) = str_field(block, &["toolUseId"]) {
                calls.insert(id.to_string(), tools.len());
            }
            tools.push(ChatToolCall {
                name: str_field(block, &["name"]).unwrap_or("tool").into(),
                timestamp: kiro_event_timestamp(data),
                summary: block
                    .get("input")
                    .map(summarize_tool)
                    .unwrap_or_else(|| "tool call".into()),
            });
        }
    } else if kind == Some("ToolResults") {
        let blocks = data
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten();
        for block in blocks {
            if str_field(block, &["kind"]) != Some("toolResult") {
                continue;
            }
            let Some(result) = block.get("data") else {
                continue;
            };
            let outcome = result.get("content").and_then(text_from_value);
            match str_field(result, &["toolUseId"]).and_then(|id| calls.get(id)) {
                Some(&index) => {
                    if let Some(outcome) = outcome {
                        tools[index].summary = outcome;
                    }
                }
                None => tools.push(ChatToolCall {
                    name: "tool".into(),
                    timestamp: kiro_event_timestamp(data),
                    summary: outcome.unwrap_or_else(|| "tool results".into()),
                }),
            }
        }
    }
}

fn tool_from_event(value: &Value) -> Option<ChatToolCall> {
    let event = value.get("payload").unwrap_or(value);
    let nested = event
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
        .and_then(|blocks| {
            blocks
                .iter()
                .find(|block| str_field(block, &["type"]) == Some("tool_use"))
        })
        .map(ToOwned::to_owned);
    let tool = match &nested {
        Some(block) => block,
        None => event
            .get("tool_call")
            .or_else(|| event.get("toolCall"))
            .or_else(|| event.get("toolCalls"))
            .or_else(|| event.get("tool_use"))?,
    };
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
    if let Some(path) = env::var_os("AGENT_SWITCH_DATA_DIR").filter(|value| !value.is_empty()) {
        return PathBuf::from(path);
    }
    dirs::data_local_dir()
        .map(|path| path.join("AgentSwitch"))
        .or_else(|| dirs::home_dir().map(|path| path.join(".agentswitch")))
        .unwrap_or_else(|| env::temp_dir().join("AgentSwitch"))
}

fn move_path(source: &Path, target: &Path) -> Result<()> {
    crate::config_store::move_path(source, target)
}

fn available_restore_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("chat");
    let suffix = path
        .extension()
        .and_then(|s| s.to_str())
        .map_or(String::new(), |ext| format!(".{ext}"));
    (2u32..)
        .map(|n| parent.join(format!("{stem}-restored-{n}{suffix}")))
        .find(|candidate| !candidate.exists())
        .expect("the candidate sequence is infinite")
}

fn project_label_from_path(provider: ChatProvider, _root: &Path, path: &Path) -> String {
    match provider {
        ChatProvider::Claude => path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .map(decode_claude_project_slug)
            .unwrap_or_else(|| "Claude project".into()),
        _ => path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| provider.label().into()),
    }
}

fn first_string(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_array)
        .and_then(|arr| arr.iter().find_map(Value::as_str))
}

fn decode_claude_project_slug(slug: &str) -> String {
    fn render(slug: &str, keep_word_hyphens: bool) -> String {
        let sep = std::path::MAIN_SEPARATOR;
        let (drive, rest) = match slug.as_bytes() {
            [letter, b'-', ..] if letter.is_ascii_alphabetic() => {
                (format!("{}:", *letter as char), &slug[2..])
            }
            _ => (String::new(), slug),
        };
        let mut out = drive;
        let mut first_run = true;
        let mut chars = rest.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '-' {
                out.push(c);
                continue;
            }
            let mut run = 1usize;
            while chars.peek() == Some(&'-') {
                chars.next();
                run += 1;
            }
            if !first_run && keep_word_hyphens && run == 1 {
                out.push('-');
            } else {
                out.push(sep);
            }
            first_run = false;
        }
        out
    }

    let relaxed = render(slug, true);
    if std::path::Path::new(&relaxed).is_dir()
        || !std::path::Path::new(&render(slug, false)).is_dir()
    {
        return relaxed;
    }
    render(slug, false)
}

fn kiro_sessions_dir() -> Result<PathBuf> {
    kiro_sessions_dir_from(crate::provider::env_path("KIRO_HOME"))
}

fn kiro_sessions_dir_from(override_home: Option<PathBuf>) -> Result<PathBuf> {
    let base = override_home
        .or_else(|| dirs::home_dir().map(|home| home.join(".kiro")))
        .ok_or_else(|| anyhow!("cannot resolve Kiro home"))?;
    Ok(base.join("sessions").join("cli"))
}

fn restore_claude_native(archive: &ChatArchive, project_dir: Option<&Path>) -> Result<PathBuf> {
    let home = claude_home().ok_or_else(|| anyhow!("cannot resolve Claude home"))?;
    let cwd = project_dir
        .map(|p| p.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| archive.project_path.clone());
    let project_id = claude_project_slug(&cwd);
    let dir = home.join("projects").join(&project_id);
    fs::create_dir_all(&dir)?;
    let session_id = gen_uuid();
    let path = dir.join(format!("{session_id}.jsonl"));

    let mut log = String::new();
    if archive.raw_events.is_empty() {
        let now = fmt_iso(unix_now());
        let mut previous_uuid: Option<String> = None;
        for (index, msg) in archive.messages.iter().enumerate() {
            let ts = msg.timestamp.as_deref().unwrap_or(&now);
            let role = if msg.role == "assistant" {
                "assistant"
            } else {
                "user"
            };
            let uuid = format!("{session_id}-{index}");
            let mut ev = serde_json::json!({
                "parentUuid": previous_uuid,
                "isSidechain": false,
                "type": role,
                "message": {"role": role, "content": [{"type": "text", "text": msg.text}]},
                "sessionId": session_id,
                "uuid": &uuid,
                "timestamp": ts,
                "cwd": cwd,
            });
            if role == "user" {
                ev["userType"] = serde_json::json!("external");
            }
            previous_uuid = Some(uuid);
            log.push_str(&serde_json::to_string(&ev)?);
            log.push('\n');
        }
    } else {
        for ev in &archive.raw_events {
            log.push_str(&serde_json::to_string(ev)?);
            log.push('\n');
        }
    }
    atomic_write(&path, log.as_bytes())?;
    Ok(path)
}

fn claude_project_slug(cwd: &str) -> String {
    let slug: String = cwd
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | ' ' => '-',
            _ => c,
        })
        .collect();
    if slug.is_empty() {
        "unknown-project".into()
    } else {
        slug
    }
}

fn opencode_project_id(directory: &str) -> String {
    let slug: String = directory
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    format!("proj_{}", slug.trim_matches('-'))
}

fn restore_kiro_native(archive: &ChatArchive, project_dir: Option<&Path>) -> Result<PathBuf> {
    let root = kiro_sessions_dir()?;
    fs::create_dir_all(&root)?;
    let id = gen_uuid();
    let cwd = project_dir
        .map(|p| p.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| archive.project_path.clone());
    let now = fmt_iso(unix_now());
    let session_state = archive.raw_events.iter()
        .find_map(|v| v.get("__agentswitch_kiro_meta")?.get("session_state").cloned())
        .unwrap_or_else(|| serde_json::json!({"version":"v1","conversation_metadata":{"user_turn_metadatas":[],"user_turn_start_request":null,"last_request":null}}));

    let meta = serde_json::json!({
        "session_id": id,
        "cwd": cwd,
        "created_at": archive.created_at.clone().unwrap_or_else(|| now.clone()),
        "updated_at": archive.updated_at.clone().unwrap_or_else(|| now.clone()),
        "title": archive.title,
        "session_state": session_state,
    });
    let json_path = root.join(format!("{id}.json"));
    fs::write(&json_path, serde_json::to_string_pretty(&meta)?)?;

    let mut log = String::new();
    if archive.raw_events.is_empty() {
        let now_secs = unix_now() as i64;
        for msg in &archive.messages {
            let kind = match msg.role.as_str() {
                "user" => "Prompt",
                "assistant" => "AssistantMessage",
                "tool" => "ToolResults",
                _ => continue,
            };
            let timestamp = msg
                .timestamp
                .as_deref()
                .and_then(parse_epoch_millis)
                .map(|ms| ms / 1000)
                .unwrap_or(now_secs);
            let block_kind = if kind == "ToolResults" {
                "toolResult"
            } else {
                "text"
            };
            let mut data = serde_json::json!({
                "message_id": gen_uuid(),
                "content": [{"kind": block_kind, "data": msg.text}],
            });
            if kind == "Prompt" {
                data["meta"] = serde_json::json!({"timestamp": timestamp});
            }
            let ev = serde_json::json!({
                "version": "v1",
                "kind": kind,
                "data": data,
            });
            log.push_str(&serde_json::to_string(&ev)?);
            log.push('\n');
        }
    } else {
        for ev in &archive.raw_events {
            if ev.get("__agentswitch_kiro_meta").is_some() {
                continue;
            }
            log.push_str(&serde_json::to_string(ev)?);
            log.push('\n');
        }
    }
    fs::write(root.join(format!("{id}.jsonl")), log)?;
    Ok(json_path)
}

fn restore_codex_native(archive: &ChatArchive, project_dir: Option<&Path>) -> Result<PathBuf> {
    let home = codex_home().ok_or_else(|| anyhow!("no home dir"))?;
    let cwd = project_dir
        .map(|p| p.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| archive.project_path.clone());
    let id = gen_uuid();
    let now_iso = fmt_iso(unix_now());
    let date_part = &now_iso[..10];
    let parts: Vec<&str> = date_part.split('-').collect();
    let dir = home
        .join("sessions")
        .join(parts[0])
        .join(parts[1])
        .join(parts[2]);
    fs::create_dir_all(&dir)?;

    let ts_file = now_iso[..19].replace(':', "-");
    let filename = format!("rollout-{ts_file}-{id}.jsonl");
    let rollout_path = dir.join(&filename);

    let mut log = String::new();
    let mut registered_id = id.clone();
    if archive.raw_events.is_empty() {
        let meta_line = serde_json::json!({
            "timestamp": &now_iso,
            "type": "session_meta",
            "payload": {
                "session_id": &id,
                "id": &id,
                "timestamp": &now_iso,
                "cwd": &cwd,
                "originator": "codex_cli_rs",
                "cli_version": "0.0.0",
                "source": "cli",
                "thread_source": "user",
                "model_provider": "openai",
                "base_instructions": null,
                "dynamic_tools": null,
                "history_mode": "legacy",
                "context_window": null,
            }
        });
        log.push_str(&serde_json::to_string(&meta_line)?);
        log.push('\n');
        for (msg_index, msg) in archive.messages.iter().enumerate() {
            let (role, content_type, event_type) = match msg.role.as_str() {
                "user" => ("user", "input_text", "user_message"),
                "assistant" => ("assistant", "output_text", "agent_message"),
                _ => continue,
            };
            let ts = msg.timestamp.as_deref().unwrap_or(&now_iso);
            let item = serde_json::json!({
                "timestamp": ts,
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "id": format!("msg_{registered_id}-{msg_index}"),
                    "role": role,
                    "content": [{"type": content_type, "text": &msg.text}],
                }
            });
            log.push_str(&serde_json::to_string(&item)?);
            log.push('\n');
            let echo = serde_json::json!({
                "timestamp": ts,
                "type": "event_msg",
                "payload": {"type": event_type, "message": &msg.text}
            });
            log.push_str(&serde_json::to_string(&echo)?);
            log.push('\n');
        }
    } else {
        for ev in &archive.raw_events {
            log.push_str(&serde_json::to_string(ev)?);
            log.push('\n');
        }
        for ev in &archive.raw_events {
            if ev.get("type").and_then(|t| t.as_str()) == Some("session_meta") {
                if let Some(found) = ev
                    .pointer("/payload/session_id")
                    .or_else(|| ev.pointer("/payload/id"))
                    .and_then(|v| v.as_str())
                {
                    registered_id = found.to_string();
                }
                break;
            }
        }
    }
    atomic_write(&rollout_path, log.as_bytes())?;

    let index_path = home.join("session_index.jsonl");
    let entry = serde_json::json!({"id": &registered_id, "thread_name": &archive.title, "updated_at": &now_iso});
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&index_path)?;
    writeln!(f, "{}", serde_json::to_string(&entry)?)?;

    let first_user = archive
        .messages
        .iter()
        .find(|m| m.role == "user")
        .map(|m| m.text.as_str());
    codex_state_register(
        &rollout_path,
        &registered_id,
        &cwd,
        &archive.title,
        first_user,
        archive.created_at.as_deref(),
    )?;

    Ok(rollout_path)
}

fn codex_state_db_path() -> Option<PathBuf> {
    let home = codex_home()?;
    let mut best: Option<(u32, PathBuf)> = None;
    for entry in fs::read_dir(home).ok()?.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(rest) = name
            .strip_prefix("state_")
            .and_then(|r| r.strip_suffix(".sqlite"))
        else {
            continue;
        };
        let Ok(version) = rest.parse::<u32>() else {
            continue;
        };
        if best.as_ref().map_or(true, |(top, _)| version > *top) {
            best = Some((version, entry.path()));
        }
    }
    best.map(|(_, path)| path)
}

fn codex_state_cwd(cwd: &str) -> String {
    if cfg!(windows) && !cwd.starts_with("\\\\?\\") && Path::new(cwd).is_absolute() {
        format!("\\\\?\\{cwd}")
    } else {
        cwd.to_string()
    }
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        text.chars().take(max).collect()
    }
}

#[allow(clippy::too_many_arguments)]
fn codex_state_register(
    rollout_path: &Path,
    session_id: &str,
    cwd: &str,
    title: &str,
    first_user_message: Option<&str>,
    created_label: Option<&str>,
) -> Result<()> {
    let Some(db_path) = codex_state_db_path() else {
        return Ok(());
    };
    let now_secs = unix_now() as i64;
    let created_ms = created_label
        .and_then(parse_epoch_millis)
        .unwrap_or(now_secs * 1000);
    let preview = truncate_chars(first_user_message.unwrap_or(title), 1000);
    let conn = Connection::open(&db_path)?;
    conn.busy_timeout(std::time::Duration::from_secs(3))?;
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| -> Result<()> {
        conn.execute(
            "INSERT OR IGNORE INTO threads (id, rollout_path, created_at, updated_at, source, model_provider, cwd, title, sandbox_policy, approval_mode, first_user_message, preview, recency_at, recency_at_ms, created_at_ms, updated_at_ms, thread_source) \
             VALUES (?1, ?2, ?3, ?4, 'cli', 'openai', ?5, ?6, '{\"type\":\"read-only\"}', 'never', ?7, ?7, ?4, ?8, ?9, ?8, 'user')",
            rusqlite::params![
                session_id,
                rollout_path.to_string_lossy(),
                created_ms / 1000,
                now_secs,
                codex_state_cwd(cwd),
                truncate_chars(title, 1000),
                preview,
                now_secs * 1000,
                created_ms,
            ],
        )?;
        Ok(())
    })();
    finish_tx(&conn, result)
}

fn codex_state_unregister(rollout_path: &Path) {
    let Some(db_path) = codex_state_db_path() else {
        return;
    };
    let Ok(conn) = Connection::open(&db_path) else {
        return;
    };
    let _ = conn.busy_timeout(std::time::Duration::from_secs(3));
    let _ = conn.execute(
        "DELETE FROM threads WHERE rollout_path = ?1",
        rusqlite::params![rollout_path.to_string_lossy()],
    );
}

fn restore_opencode_native(
    archive: &ChatArchive,
    project_dir: Option<&Path>,
    db_path: &Path,
) -> Result<PathBuf> {
    insert_opencode_session(db_path, archive, project_dir, None, None, None)
}

fn insert_opencode_session(
    db_path: &Path,
    archive: &ChatArchive,
    project_dir: Option<&Path>,
    session_id: Option<&str>,
    created_ms: Option<i64>,
    updated_ms: Option<i64>,
) -> Result<PathBuf> {
    let conn = Connection::open(db_path)?;
    conn.busy_timeout(std::time::Duration::from_secs(3))?;
    let requested = session_id.filter(|id| !id.is_empty());
    let id = match requested {
        Some(requested)
            if conn
                .query_row(
                    "SELECT COUNT(*) FROM session WHERE id = ?1",
                    rusqlite::params![requested],
                    |row| row.get::<_, i64>(0),
                )
                .map(|count| count > 0)
                .unwrap_or(false) =>
        {
            gen_uuid()
        }
        Some(requested) => requested.to_string(),
        None => gen_uuid(),
    };
    let dir = project_dir
        .map(|p| p.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| archive.project_path.clone());
    let now_ms = updated_ms.unwrap_or((unix_now() as i64) * 1000);
    let created_ms = created_ms
        .or_else(|| archive.created_at.as_deref().and_then(parse_epoch_millis))
        .unwrap_or(now_ms);
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| -> Result<()> {
        let tables = sqlite_tables(&conn)?;
        let session_columns = sqlite_columns(&conn, "session")?;
        let project_id = if dir.is_empty() {
            "imported".to_string()
        } else {
            opencode_project_id(&dir)
        };
        if project_id != "imported" && tables.contains("projects") {
            conn.execute(
                "INSERT OR IGNORE INTO projects (id, name, metadata, position, created_at_ms, updated_at_ms) VALUES (?1, ?2, '{}', 0, ?3, ?3)",
                rusqlite::params![&project_id, &dir, created_ms],
            )?;
            if tables.contains("project_roots") {
                conn.execute(
                    "INSERT OR IGNORE INTO project_roots (project_id, position, path) VALUES (?1, 0, ?2)",
                    rusqlite::params![&project_id, &dir],
                )?;
            }
        }
        let version: String = conn
            .query_row(
                "SELECT version FROM session ORDER BY time_updated DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap_or_else(|_| "1".to_string());
        let mut columns: Vec<(&str, String)> = vec![
            ("id", id.clone()),
            ("project_id", project_id.clone()),
            ("slug", safe_file_stem(&archive.title)),
            ("directory", dir.clone()),
            ("title", archive.title.clone()),
            ("version", version),
            ("time_created", created_ms.to_string()),
            ("time_updated", now_ms.to_string()),
        ];
        if session_columns.contains("path") {
            columns.push(("path", dir.clone()));
        }
        if session_columns.contains("task_type") {
            columns.push(("task_type", "interactive".to_string()));
        }
        if session_columns.contains("title_source") {
            columns.push(("title_source", "generated".to_string()));
        }
        let mut names: Vec<&str> = Vec::with_capacity(columns.len());
        let mut values: Vec<&str> = Vec::with_capacity(columns.len());
        for (name, value) in &columns {
            names.push(*name);
            values.push(value.as_str());
        }
        let column_list = names.join(", ");
        let placeholders = (1..=columns.len())
            .map(|n| format!("?{n}"))
            .collect::<Vec<_>>()
            .join(",");
        conn.execute(
            &format!("INSERT OR IGNORE INTO session ({column_list}) VALUES ({placeholders})"),
            rusqlite::params_from_iter(values.iter()),
        )?;
        for (i, msg) in archive.messages.iter().enumerate() {
            let msg_id = format!("{id}-msg-{i}");
            let data = serde_json::json!({
                "role": &msg.role,
                "content": &msg.text,
                "time": {"created": now_ms + i as i64},
            })
            .to_string();
            conn.execute(
                "INSERT OR IGNORE INTO message (id, session_id, time_created, time_updated, data) VALUES (?1,?2,?3,?4,?5)",
                rusqlite::params![&msg_id, &id, now_ms + i as i64, now_ms + i as i64, &data],
            )?;
            let part_id = format!("{msg_id}-part-0");
            let part_data = serde_json::json!({"type": "text", "text": &msg.text}).to_string();
            conn.execute(
                "INSERT OR IGNORE INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES (?1,?2,?3,?4,?5,?6)",
                rusqlite::params![&part_id, &msg_id, &id, now_ms + i as i64, now_ms + i as i64, &part_data],
            )?;
        }
        Ok(())
    })();
    finish_tx(&conn, result)?;
    Ok(db_path.to_path_buf())
}

fn restore_db_session_from_trash(
    manifest: &DeleteManifest,
    manifest_path: &Path,
) -> Result<PathBuf> {
    let archive_path = manifest
        .trashed_path
        .as_ref()
        .ok_or_else(|| anyhow!("trashed archive missing"))?;
    let archive: ChatArchive = serde_json::from_str(&fs::read_to_string(archive_path)?)?;
    let db_path = manifest
        .original_path
        .as_ref()
        .filter(|path| path.is_file())
        .cloned()
        .or_else(|| match manifest.provider {
            ChatProvider::OpenCode => opencode_db_path(),
            ChatProvider::Zcode => zcode_db_path(),
            _ => None,
        })
        .ok_or_else(|| anyhow!("the original chat database no longer exists"))?;
    insert_opencode_session(
        &db_path,
        &archive,
        None,
        Some(&manifest.session_id),
        archive.created_at.as_deref().and_then(parse_epoch_millis),
        archive.updated_at.as_deref().and_then(parse_epoch_millis),
    )?;
    fs::remove_file(archive_path)?;
    fs::remove_file(manifest_path)?;
    Ok(db_path)
}

fn restore_zcode_native(
    archive: &ChatArchive,
    project_dir: Option<&Path>,
    db_path: &Path,
) -> Result<PathBuf> {
    restore_opencode_native(archive, project_dir, db_path)
}

fn parse_epoch_millis(label: &str) -> Option<i64> {
    if let Some(raw) = label.strip_prefix("unix:") {
        return raw.parse::<i64>().ok().map(|secs| secs * 1000);
    }
    if let Ok(value) = label.parse::<i64>() {
        return Some(if value > 1_000_000_000_000 {
            value
        } else {
            value * 1000
        });
    }
    parse_iso_seconds(label).map(|secs| secs * 1000)
}

fn gen_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn fmt_iso(secs: u64) -> String {
    let d = (secs / 86400) as i64;
    let rem = (secs % 86400) as i64;
    let z = d + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let yr = if m <= 2 { y + 1 } else { y };
    format!(
        "{yr:04}-{m:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
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
    jsonl_lines(file)
        .filter_map(|line| serde_json::from_str::<Value>(&line).ok())
        .filter_map(|value| kiro_message_from_event(&value))
        .count()
}

fn first_kiro_prompt(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    jsonl_lines(file)
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

fn codex_home() -> Option<PathBuf> {
    crate::provider::env_path("CODEX_HOME")
        .or_else(|| dirs::home_dir().map(|home| home.join(".codex")))
}

fn codex_titles() -> HashMap<String, String> {
    let Some(home) = codex_home() else {
        return HashMap::new();
    };
    let path = home.join("session_index.jsonl");
    let Ok(file) = File::open(path) else {
        return HashMap::new();
    };
    jsonl_lines(file)
        .filter_map(|line| serde_json::from_str::<Value>(&line).ok())
        .filter_map(|v| {
            Some((
                str_field(&v, &["id"])?.to_string(),
                str_field(&v, &["thread_name", "title"])?.to_string(),
            ))
        })
        .collect()
}

#[allow(dead_code)]
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

fn timestamp_label(ts: i64) -> String {
    if ts > 1_000_000_000_000 {
        format!("unix:{}", ts / 1000)
    } else {
        format!("unix:{}", ts)
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
    fn claude_project_labels_keep_word_hyphens() {
        let sep = std::path::MAIN_SEPARATOR;
        // Runs of two or more hyphens always decode to separators, and both
        // interpretations agree, so this holds on every machine.
        assert_eq!(
            decode_claude_project_slug("D--AI--AgentSwitch"),
            format!("D:{sep}AI{sep}AgentSwitch")
        );
        // Neither interpretation exists on disk, so the word-hyphen reading wins.
        assert_eq!(
            decode_claude_project_slug("Q--No-Such-Dir"),
            format!("Q:{sep}No-Such-Dir")
        );
    }

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
        with_env_var("AGENT_SWITCH_DATA_DIR", &dir.join("data"), || {
            let file = write_sample_jsonl(&dir, "trash-a", "D:/work/trash");
            let session = jsonl_session(ChatProvider::Codex, &dir, &file).unwrap();
            soft_delete(&session).unwrap();
            assert!(!file.exists());
            let trash = scan_trash(None);
            assert_eq!(trash.len(), 1);
            assert_eq!(trash[0].title, session.title);
            assert_eq!(trash[0].project_path, session.project_path);
            let restored = restore_from_trash(&trash[0]).unwrap();
            assert_eq!(restored, file);
            assert!(file.exists());
            let session = jsonl_session(ChatProvider::Codex, &dir, &file).unwrap();
            soft_delete(&session).unwrap();
            let trash = scan_trash(None);
            delete_trash_forever(&trash[0]).unwrap();
            assert!(scan_trash(None).is_empty());
        });
    }

    fn with_env_var<T>(name: &str, value: &Path, run: impl FnOnce() -> T) -> T {
        with_env_vars(&[(name, value)], run)
    }

    fn with_env_vars<T>(vars: &[(&str, &Path)], run: impl FnOnce() -> T) -> T {
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = ENV_LOCK.lock().unwrap();
        let previous: Vec<(String, Option<std::ffi::OsString>)> = vars
            .iter()
            .map(|(name, _)| ((*name).to_string(), env::var_os(name)))
            .collect();
        for (name, value) in vars {
            env::set_var(name, value);
        }
        let result = run();
        for ((name, _), (_, previous)) in vars.iter().zip(previous.iter()).rev() {
            match previous {
                Some(previous) => env::set_var(name, previous),
                None => env::remove_var(name),
            }
        }
        result
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
        let sessions = with_env_var("KIRO_HOME", &kiro_home, scan_kiro);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].title, "Kiro title");
        assert_eq!(sessions[0].turn_count, 2);
        assert!(!sessions[0].subagent);
        let archive = load_archive(&sessions[0]).unwrap();
        assert_eq!(archive.messages.len(), 2);
        assert_eq!(archive.messages[0].text, "hello kiro");
    }

    #[test]
    fn kiro_sessions_surface_globally_and_badge_subagents() {
        let dir = temp_test_dir("kiro-global");
        let cli = dir.join(".kiro").join("sessions").join("cli");
        fs::create_dir_all(&cli).unwrap();
        let id = "11111111-2222-3333-4444-555555555555";
        fs::write(
            cli.join(format!("{id}.json")),
            format!(
                r#"{{"session_id":"{id}","cwd":"C:\\elsewhere","created_at":"2026-05-01T00:00:00Z","updated_at":"2026-05-01T00:01:00Z","title":"Main chat","session_created_reason":"user","session_state":{{}}}}"#
            ),
        )
        .unwrap();
        let sub_id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        fs::write(
            cli.join(format!("{sub_id}.json")),
            format!(
                r#"{{"session_id":"{sub_id}","cwd":"C:\\elsewhere","created_at":"2026-05-01T00:02:00Z","updated_at":"2026-05-01T00:03:00Z","title":"Helper chat","session_created_reason":"subagent","session_state":{{}}}}"#
            ),
        )
        .unwrap();
        for sid in [id, sub_id] {
            fs::write(
                cli.join(format!("{sid}.jsonl")),
                r#"{"version":1,"kind":"Prompt","data":{"message_id":"u1","content":[{"kind":"text","data":"hi"}]}}"#,
            )
            .unwrap();
        }
        let sessions = with_env_var("KIRO_HOME", &dir.join(".kiro"), scan_kiro);
        assert_eq!(sessions.len(), 2);
        let by_id: std::collections::HashMap<_, _> = sessions
            .iter()
            .map(|s| (s.id.as_str(), s.subagent))
            .collect();
        assert!(!by_id[id]);
        assert!(by_id[sub_id]);
    }

    #[test]
    fn kiro_tool_use_blocks_become_tool_calls_with_outcomes() {
        let dir = temp_test_dir("kiro-tools");
        let cli = dir.join(".kiro").join("sessions").join("cli");
        fs::create_dir_all(&cli).unwrap();
        let id = "11111111-2222-3333-4444-555555555555";
        fs::write(
            cli.join(format!("{id}.json")),
            format!(
                r#"{{"session_id":"{id}","cwd":"D:\\AI","created_at":"2026-05-01T00:00:00Z","updated_at":"2026-05-01T00:01:00Z","title":"Tool chat","session_state":{{}}}}"#
            ),
        )
        .unwrap();
        fs::write(
            cli.join(format!("{id}.jsonl")),
            r#"{"kind":"Prompt","data":{"message_id":"u1","content":[{"kind":"text","data":"list files"}],"meta":{"timestamp":1787510770}}}"#
                .to_string()
                + "\n"
                + r#"{"kind":"AssistantMessage","data":{"message_id":"a1","content":[{"kind":"toolUse","data":{"toolUseId":"toolu_1","name":"glob","input":{"pattern":"*.rs"}}}]}}"#
                + "\n"
                + r#"{"kind":"ToolResults","data":{"message_id":"t1","content":[{"kind":"toolResult","data":{"toolUseId":"toolu_1","content":[{"kind":"json","data":["main.rs"]}]}}]}}"#
                + "\n"
                + r#"{"kind":"AssistantMessage","data":{"message_id":"a2","content":[{"kind":"text","data":"found main.rs"}]}}"#,
        )
        .unwrap();

        let sessions = with_env_var("KIRO_HOME", &dir.join(".kiro"), scan_kiro);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].turn_count, 3);
        let archive = load_archive(&sessions[0]).unwrap();
        assert_eq!(archive.messages.len(), sessions[0].turn_count);
        assert!(archive.messages[1].text.trim().is_empty());
        assert_eq!(archive.messages[2].text, "found main.rs");
        assert_eq!(archive.tool_calls.len(), 1);
        let call = &archive.tool_calls[0];
        assert_eq!(call.name, "glob");
        assert!(
            call.summary.contains("main.rs"),
            "outcome replaces the input summary: {}",
            call.summary
        );
    }

    #[test]
    fn failed_kiro_trash_moves_session_files_back() {
        let dir = temp_test_dir("kiro-rollback");
        let kiro_home = dir.join(".kiro");
        let cli = kiro_home.join("sessions").join("cli");
        fs::create_dir_all(&cli).unwrap();
        let id = "11111111-2222-3333-4444-555555555555";
        fs::write(
            cli.join(format!("{id}.json")),
            format!(
                r#"{{"session_id":"{id}","cwd":"D:\\AI","created_at":"2026-05-01T00:00:00Z","updated_at":"2026-05-01T00:01:00Z","title":"Rollback chat","session_state":{{}}}}"#
            ),
        )
        .unwrap();
        fs::write(
            cli.join(format!("{id}.jsonl")),
            r#"{"kind":"Prompt","data":{"message_id":"u1","content":[{"kind":"text","data":"hi"}]}}"#,
        )
        .unwrap();
        fs::write(cli.join(format!("{id}.lock")), "lock").unwrap();
        let data = dir.join("data");

        with_env_vars(
            &[("KIRO_HOME", &kiro_home), ("AGENT_SWITCH_DATA_DIR", &data)],
            || {
                let session = scan_kiro().remove(0);
                let stem = safe_file_stem(&format!("{}-{}", session.id, session.title));
                let trash_dir = trash_dir().join("kiro");
                fs::create_dir_all(trash_dir.join(format!("{stem}.delete.json"))).unwrap();

                assert!(soft_delete(&session).is_err());
                assert!(
                    scan_trash(None).is_empty(),
                    "a failed trash must not leave a discoverable manifest"
                );
                for ext in ["json", "jsonl", "lock"] {
                    assert!(
                        cli.join(format!("{id}.{ext}")).exists(),
                        "{ext} was rolled back to its origin"
                    );
                }
            },
        );
    }

    #[test]
    fn imports_kiro_archive_back_into_native_store() {
        let dir = temp_test_dir("kiro-import");
        let kiro_home = dir.join(".kiro");
        let cli = kiro_home.join("sessions").join("cli");
        fs::create_dir_all(&cli).unwrap();
        let id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        fs::write(
            cli.join(format!("{id}.json")),
            format!(
                r#"{{"session_id":"{id}","cwd":"D:\\orig","created_at":"2026-05-01T00:00:00Z","updated_at":"2026-05-01T00:01:00Z","title":"Round trip","session_state":{{}}}}"#
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

        let sessions = with_env_var("KIRO_HOME", &kiro_home, scan_kiro);
        assert_eq!(sessions.len(), 1);

        let archive_path = dir.join("kiro-export.agentswitch-chat.json");
        export_session(&sessions[0], &archive_path).unwrap();

        let project = dir.join("project");
        fs::create_dir_all(&project).unwrap();
        let restored_json = with_env_var("KIRO_HOME", &kiro_home, || {
            import_archive(&archive_path, Some(&project)).unwrap()
        });

        let new_id = restored_json
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap()
            .to_string();
        assert_ne!(new_id, id);
        assert!(cli.join(format!("{new_id}.json")).exists());
        assert!(cli.join(format!("{new_id}.jsonl")).exists());

        let imported = kiro_session(&cli.join(format!("{new_id}.json"))).unwrap();
        assert_eq!(imported.project_path, project.to_string_lossy().to_string());
        assert_eq!(imported.turn_count, 2);
        assert_eq!(imported.title, "Round trip");

        let restored_archive = load_archive(&imported).unwrap();
        assert_eq!(restored_archive.messages.len(), 2);
        assert_eq!(restored_archive.messages[0].text, "hello kiro");
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

    #[test]
    fn empty_or_relative_home_overrides_fall_back_to_defaults() {
        let home = dirs::home_dir().unwrap();
        with_env_vars(
            &[
                ("KIRO_HOME", Path::new("")),
                ("CLAUDE_CONFIG_DIR", Path::new("")),
                ("CODEX_HOME", Path::new("relative/path")),
            ],
            || {
                assert_eq!(
                    kiro_sessions_dir().unwrap(),
                    home.join(".kiro").join("sessions").join("cli")
                );
                assert_eq!(claude_home().unwrap(), home.join(".claude"));
                assert_eq!(codex_home().unwrap(), home.join(".codex"));
            },
        );
    }

    #[test]
    fn rejects_zip_with_too_many_entries() {
        let dir = temp_test_dir("zip-limit");
        let path = dir.join("too-many.zip");
        let file = File::create(&path).unwrap();
        let mut zip = ZipWriter::new(file);
        let options = SimpleFileOptions::default();
        for index in 0..=MAX_ZIP_ENTRIES {
            zip.start_file(format!("ignored-{index}.txt"), options)
                .unwrap();
        }
        zip.finish().unwrap();

        let error = import_zip(&path, None).unwrap_err().to_string();
        assert!(error.contains("too many entries"));
    }

    #[test]
    fn scans_opencode_sessions_from_db() {
        let dir = temp_test_dir("opencode-db");
        let db_path = dir.join("opencode.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                slug TEXT NOT NULL,
                directory TEXT NOT NULL,
                title TEXT NOT NULL,
                version TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL
            );
            CREATE TABLE message (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                data TEXT NOT NULL
            );
            CREATE TABLE part (
                id TEXT PRIMARY KEY,
                message_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                data TEXT NOT NULL
            );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session (id, project_id, slug, directory, title, version, time_created, time_updated) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                "ses_test1",
                "proj1",
                "test-session",
                "/home/user/project",
                "Test OpenCode Chat",
                "1",
                1780000000000_i64,
                1780001000000_i64
            ],
        ).unwrap();
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                "msg1",
                "ses_test1",
                1780000100000_i64,
                1780000100000_i64,
                r#"{"role":"user"}"#
            ],
        ).unwrap();
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                "msg2",
                "ses_test1",
                1780000200000_i64,
                1780000200000_i64,
                r#"{"role":"assistant"}"#
            ],
        ).unwrap();
        conn.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                "part1",
                "msg1",
                "ses_test1",
                1780000100000_i64,
                1780000100000_i64,
                r#"{"type":"text","text":"hello"}"#
            ],
        ).unwrap();
        conn.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                "part2",
                "msg2",
                "ses_test1",
                1780000200000_i64,
                1780000200000_i64,
                r#"{"type":"text","text":"hi there"}"#
            ],
        ).unwrap();
        conn.close().unwrap();

        let sessions = with_env_var("OPENCODE_DB", &db_path, scan_opencode);

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].title, "Test OpenCode Chat");
        assert_eq!(sessions[0].project_path, "/home/user/project");
        assert_eq!(sessions[0].turn_count, 2);
        assert_eq!(sessions[0].provider, ChatProvider::OpenCode);

        let archive = load_archive(&sessions[0]).unwrap();
        assert_eq!(archive.messages.len(), 2);
        assert_eq!(archive.messages[0].role, "user");
        assert_eq!(archive.messages[0].text, "hello");
        assert_eq!(archive.messages[1].role, "assistant");
        assert_eq!(archive.messages[1].text, "hi there");
    }

    #[test]
    fn opencode_keeps_messages_with_identical_metadata_separate() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE message (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                data TEXT NOT NULL
            );
            CREATE TABLE part (
                id TEXT PRIMARY KEY,
                message_id TEXT NOT NULL,
                data TEXT NOT NULL
            );
            INSERT INTO message VALUES ('msg1', 'session1', 1, '{\"role\":\"assistant\"}');
            INSERT INTO message VALUES ('msg2', 'session1', 2, '{\"role\":\"assistant\"}');
            INSERT INTO part VALUES ('part1', 'msg1', '{\"type\":\"text\",\"text\":\"first\"}');
            INSERT INTO part VALUES ('part2', 'msg2', '{\"type\":\"text\",\"text\":\"second\"}');",
        )
        .unwrap();
        let mut messages = Vec::new();
        let mut tools = Vec::new();

        load_opencode_current(&conn, "session1", &mut messages, &mut tools).unwrap();

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].text, "first");
        assert_eq!(messages[1].text, "second");
    }

    #[test]
    fn codex_event_msg_echoes_do_not_duplicate_response_items() {
        let dir = temp_test_dir("codex-dedup");
        let file = dir.join("rollout-dup.jsonl");
        fs::write(
            &file,
            r#"{"timestamp":"2026-05-01T00:00:00Z","type":"session_meta","payload":{"id":"dup1","cwd":"D:/work/dup"}}"#.to_string()
                + "\n"
                + r#"{"timestamp":"2026-05-01T00:01:00Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"build it"}]}}"#
                + "\n"
                + r#"{"timestamp":"2026-05-01T00:01:01Z","type":"event_msg","payload":{"type":"user_message","message":"build it"}}"#
                + "\n"
                + r#"{"timestamp":"2026-05-01T00:01:02Z","type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"context dump"}]}}"#,
        )
        .unwrap();
        let session = jsonl_session(ChatProvider::Codex, &dir, &file).unwrap();
        assert_eq!(
            session.turn_count, 1,
            "echo and developer turns are not counted"
        );
        let archive = load_archive(&session).unwrap();
        assert_eq!(archive.messages.len(), 1);
        assert_eq!(archive.messages[0].role, "user");
        assert_eq!(archive.messages[0].text, "build it");
    }

    #[test]
    fn claude_restore_uses_slug_directory_and_native_events() {
        let dir = temp_test_dir("claude-restore");
        let home = dir.join(".claude");
        let archive = ChatArchive {
            schema_version: ARCHIVE_VERSION,
            source_provider: ChatProvider::Claude,
            source_session_id: "src".into(),
            title: "Restored chat".into(),
            project_path: r"D:\AI\FFmpeg-TUI".into(),
            created_at: None,
            updated_at: None,
            messages: vec![
                ChatMessage {
                    role: "user".into(),
                    timestamp: Some("2026-05-01T00:00:00Z".into()),
                    text: "hello from the archive".into(),
                },
                ChatMessage {
                    role: "assistant".into(),
                    timestamp: Some("2026-05-01T00:01:00Z".into()),
                    text: "glad to help".into(),
                },
            ],
            tool_calls: vec![],
            raw_events: vec![],
        };
        with_env_var("CLAUDE_CONFIG_DIR", &home, || {
            let path = restore_claude_native(&archive, None).unwrap();
            let parent = path
                .parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_str()
                .unwrap();
            assert_eq!(
                parent, r"D--AI-FFmpeg-TUI",
                "directory matches Claude's slug scheme"
            );
            let raw = std::fs::read_to_string(&path).unwrap();
            let events: Vec<serde_json::Value> = raw
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            let first = &events[0];
            assert_eq!(first["type"], "user");
            assert_eq!(first["message"]["role"], "user");
            assert_eq!(
                first["message"]["content"][0]["text"],
                "hello from the archive"
            );
            assert_eq!(
                first["parentUuid"],
                serde_json::Value::Null,
                "first turn has no parent"
            );
            assert_eq!(first["isSidechain"], false);
            assert_eq!(first["userType"], "external");
            let second = &events[1];
            assert_eq!(second["type"], "assistant");
            assert_eq!(second["parentUuid"], first["uuid"]);
            assert!(second.get("userType").is_none());
        });
    }

    #[test]
    fn timestamp_sort_key_orders_unix_and_iso_labels() {
        assert!(timestamp_sort_key("unix:1780617700") > timestamp_sort_key("2026-06-05T00:00:00Z"));
        assert!(timestamp_sort_key("unix:1780617500") < timestamp_sort_key("2026-06-05T00:00:00Z"));
        assert!(
            timestamp_sort_key("unix:999999999") < timestamp_sort_key("unix:10000000000"),
            "numeric compare, not lexicographic"
        );
        assert!(timestamp_sort_key("unknown") == i64::MIN);
    }

    #[test]
    fn scans_zcode_sessions_from_sqlite_db() {
        let dir = temp_test_dir("zcode-db");
        let db_path = dir.join("db.sqlite");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                slug TEXT NOT NULL,
                directory TEXT NOT NULL,
                title TEXT NOT NULL,
                version TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL
            );
            CREATE TABLE message (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                data TEXT NOT NULL
            );
            CREATE TABLE part (
                id TEXT PRIMARY KEY,
                message_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                data TEXT NOT NULL
            );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session (id, project_id, slug, directory, title, version, time_created, time_updated) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            rusqlite::params![
                "sess_abc",
                "proj_d-work-app",
                "sess_abc",
                "D:/work/app",
                "ZCode session",
                "1",
                1780000000000_i64,
                1780001000000_i64
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?1,?2,?3,?4,?5)",
            rusqlite::params!["m1", "sess_abc", 1780000100000_i64, 1780000100000_i64, r#"{"role":"user"}"#],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES (?1,?2,?3,?4,?5,?6)",
            rusqlite::params!["p1", "m1", "sess_abc", 1780000100000_i64, 1780000100000_i64, r#"{"type":"text","text":"hi zcode"}"#],
        )
        .unwrap();
        conn.close().unwrap();

        let sessions = with_env_var("ZCODE_DB", &db_path, scan_zcode);

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].provider, ChatProvider::Zcode);
        assert_eq!(sessions[0].title, "ZCode session");
        assert_eq!(sessions[0].project_path, "D:/work/app");

        let archive = load_archive(&sessions[0]).unwrap();
        assert_eq!(archive.source_provider, ChatProvider::Zcode);
        assert_eq!(archive.messages.len(), 1);
        assert_eq!(archive.messages[0].role, "user");
        assert_eq!(archive.messages[0].text, "hi zcode");
    }

    #[test]
    fn zcode_import_restores_into_native_db() {
        let dir = temp_test_dir("zcode-import");
        let db_path = dir.join("db.sqlite");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                slug TEXT NOT NULL,
                directory TEXT NOT NULL,
                title TEXT NOT NULL,
                version TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL
            );
            CREATE TABLE message (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                data TEXT NOT NULL
            );
            CREATE TABLE part (
                id TEXT PRIMARY KEY,
                message_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                data TEXT NOT NULL
            );",
        )
        .unwrap();
        conn.close().unwrap();

        let archive_path = dir.join("zcode.agentswitch-chat.json");
        fs::write(
            &archive_path,
            serde_json::to_string_pretty(&ChatArchive {
                schema_version: ARCHIVE_VERSION,
                source_provider: ChatProvider::Zcode,
                source_session_id: "orig".into(),
                title: "Imported ZCode chat".into(),
                project_path: String::new(),
                created_at: None,
                updated_at: None,
                messages: vec![
                    ChatMessage {
                        role: "user".into(),
                        timestamp: None,
                        text: "one".into(),
                    },
                    ChatMessage {
                        role: "assistant".into(),
                        timestamp: None,
                        text: "two".into(),
                    },
                ],
                tool_calls: vec![],
                raw_events: vec![],
            })
            .unwrap(),
        )
        .unwrap();

        with_env_var("ZCODE_DB", &db_path, || {
            import_archive(&archive_path, None).unwrap();
        });

        let sessions = with_env_var("ZCODE_DB", &db_path, scan_zcode);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].title, "Imported ZCode chat");
        let archive = load_archive(&sessions[0]).unwrap();
        assert_eq!(archive.messages.len(), 2);
        assert_eq!(archive.messages[0].text, "one");
    }

    fn claude_fixture_lines(session_id: &str) -> String {
        r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"hello claude"}]},"sessionId":"SESSION","uuid":"u1","timestamp":"2026-05-01T00:00:00Z","cwd":"D:\\AI\\Demo"}"#
            .replace("SESSION", session_id)
            + "\n"
            + &r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hi there"}]},"sessionId":"SESSION","uuid":"u2","parentUuid":"u1","timestamp":"2026-05-01T00:01:00Z","cwd":"D:\\AI\\Demo"}"#
                .replace("SESSION", session_id)
    }

    #[test]
    fn claude_round_trip_scan_export_import_rescan() {
        let dir = temp_test_dir("claude-roundtrip");
        let home_a = dir.join("home-a");
        let project_dir = home_a.join("projects").join("D--AI-Demo");
        fs::create_dir_all(&project_dir).unwrap();
        fs::write(
            project_dir.join("11111111-1111-1111-1111-111111111111.jsonl"),
            claude_fixture_lines("11111111-1111-1111-1111-111111111111"),
        )
        .unwrap();

        let sessions = with_env_var("CLAUDE_CONFIG_DIR", &home_a, scan_claude);
        assert_eq!(sessions.len(), 1, "real Claude layout is discovered");
        assert_eq!(sessions[0].project_path, r"D:\AI\Demo");
        assert_eq!(sessions[0].turn_count, 2);

        let archive_path = dir.join("claude.agentswitch-chat.json");
        export_session(&sessions[0], &archive_path).unwrap();
        let home_b = dir.join("home-b");
        let restored = with_env_var("CLAUDE_CONFIG_DIR", &home_b, || {
            import_archive(&archive_path, None).unwrap()
        });
        let restored_parent = restored
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert_eq!(restored_parent, "D--AI-Demo", "slug directory matches cwd");

        let rescan = with_env_var("CLAUDE_CONFIG_DIR", &home_b, scan_claude);
        assert_eq!(rescan.len(), 1, "restored session is discoverable");
        assert_eq!(rescan[0].project_path, r"D:\AI\Demo");
        let archive = load_archive(&rescan[0]).unwrap();
        assert_eq!(archive.messages.len(), 2);
        assert_eq!(archive.messages[0].text, "hello claude");
    }

    #[test]
    fn codex_round_trip_honors_codex_home() {
        let dir = temp_test_dir("codex-roundtrip");
        let home_a = dir.join("codex-home-a");
        let sessions_dir = home_a.join("sessions").join("2026").join("05").join("01");
        fs::create_dir_all(&sessions_dir).unwrap();
        fs::write(
            sessions_dir.join("rollout-2026-05-01T00-00-00-22222222-2222-2222-2222-222222222222.jsonl"),
            r#"{"timestamp":"2026-05-01T00:00:00Z","type":"session_meta","payload":{"id":"22222222-2222-2222-2222-222222222222","cwd":"D:/work/rt"}}"#.to_string()
                + "\n"
                + r#"{"timestamp":"2026-05-01T00:01:00Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"run the tests"}]}}"#,
        )
        .unwrap();

        let sessions = with_env_var("CODEX_HOME", &home_a, scan_codex);
        assert_eq!(sessions.len(), 1);

        let archive_path = dir.join("codex.agentswitch-chat.json");
        export_session(&sessions[0], &archive_path).unwrap();

        let home_b = dir.join("codex-home-b");
        with_env_var("CODEX_HOME", &home_b, || {
            import_archive(&archive_path, None).unwrap();
        });

        let found: Vec<PathBuf> = WalkDir::new(home_b.join("sessions"))
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .map(|e| e.into_path())
            .collect();
        assert_eq!(
            found.len(),
            1,
            "exactly one restored rollout under CODEX_HOME"
        );
        assert!(home_b.join("session_index.jsonl").exists());

        let rescan = with_env_var("CODEX_HOME", &home_b, scan_codex);
        assert_eq!(rescan.len(), 1);
        let archive = load_archive(&rescan[0]).unwrap();
        assert_eq!(archive.messages.len(), 1);
        assert_eq!(archive.messages[0].text, "run the tests");
    }

    #[test]
    fn deflated_zip_export_import_round_trip() {
        let dir = temp_test_dir("zip-roundtrip");
        let data = dir.join("data");
        with_env_var("AGENT_SWITCH_DATA_DIR", &data, || {
            let make_archive = |id: &str, text: &str| ChatArchive {
                schema_version: ARCHIVE_VERSION,
                source_provider: ChatProvider::Antigravity,
                source_session_id: id.into(),
                title: format!("Archive {id}"),
                project_path: "D:/work/agy".into(),
                created_at: None,
                updated_at: None,
                messages: vec![ChatMessage {
                    role: "user".into(),
                    timestamp: None,
                    text: text.into(),
                }],
                tool_calls: vec![],
                raw_events: vec![],
            };
            let a = make_archive("agy-1", "first");
            let b = make_archive("agy-2", "second");

            let staging = dir.join("out.zip");
            let file = File::create(&staging).unwrap();
            let mut zip = ZipWriter::new(file);
            let options =
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            for (name, archive) in [
                ("chats/a.agentswitch-chat.json", &a),
                ("chats/b.agentswitch-chat.json", &b),
            ] {
                zip.start_file(name, options).unwrap();
                zip.write_all(serde_json::to_string_pretty(archive).unwrap().as_bytes())
                    .unwrap();
            }
            zip.finish().unwrap();

            let report = import_zip(&staging, None).unwrap();
            assert_eq!(report.ok, 2, "deflated entries import");
            assert_eq!(report.failed, 0);

            let imports: Vec<_> = fs::read_dir(imports_dir()).unwrap().flatten().collect();
            assert_eq!(imports.len(), 2, "neutral archives land in imports");

            let scanned = scan_imported();
            assert_eq!(scanned.len(), 2);
            assert!(scanned.iter().all(|s| s.imported));
            assert_eq!(
                scanned.iter().map(|s| s.provider).collect::<Vec<_>>(),
                vec![ChatProvider::Antigravity, ChatProvider::Antigravity]
            );
            let archive = load_archive(&scanned[0]).unwrap();
            assert_eq!(archive.source_provider, ChatProvider::Antigravity);
        });
    }

    #[test]
    #[ignore = "writes into the local Claude Code and Codex stores"]
    fn live_convert_claude_chat_to_codex_resume() {
        let sessions = scan_claude();
        assert!(!sessions.is_empty(), "expected real Claude sessions here");
        let source = &sessions[0];
        let path = convert_session(source, ChatProvider::Codex)
            .expect("conversion into the live Codex store");
        eprintln!("converted '{}' -> {}", source.title, path.display());
        if let Some(db) = codex_state_db_path() {
            let conn = Connection::open(&db).unwrap();
            let listed: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM threads WHERE rollout_path = ?1",
                    rusqlite::params![path.to_string_lossy()],
                    |row| row.get(0),
                )
                .unwrap();
            eprintln!("threads rows pointing at the converted rollout: {listed}");
            assert_eq!(listed, 1, "/resume database lists the converted chat");
        }
    }

    #[test]
    #[ignore = "requires the local Claude Code store"]
    fn live_claude_real_store_scan_and_export() {
        let Some(home) = dirs::home_dir() else {
            return;
        };
        if !home.join(".claude").join("projects").is_dir() {
            eprintln!("no live ~/.claude/projects; skipping");
            return;
        }
        let sessions = scan_claude();
        eprintln!("live Claude sessions discovered: {}", sessions.len());
        assert!(!sessions.is_empty(), "expected real Claude sessions here");
        let target =
            std::env::temp_dir().join(format!("agentswitch-live-claude-{}.json", unix_now()));
        export_session(&sessions[0], &target).unwrap();
        let bytes = fs::read_to_string(&target).unwrap();
        let archive: ChatArchive = serde_json::from_str(&bytes).unwrap();
        eprintln!(
            "exported '{}' ({} messages, {} raw events)",
            archive.title,
            archive.messages.len(),
            archive.raw_events.len()
        );
        assert!(!bytes.is_empty());
        let _ = fs::remove_file(&target);
    }

    #[test]
    #[ignore = "requires the local ZCode database"]
    fn live_zcode_real_db_snapshot_scan_and_load() {
        let home = match dirs::home_dir() {
            Some(home) => home,
            None => return,
        };
        let db = home.join(".zcode").join("cli").join("db").join("db.sqlite");
        if !db.is_file() {
            eprintln!("no live ZCode db; skipping");
            return;
        }
        let dir = temp_test_dir("live-zcode");
        for suffix in ["", "-wal", "-shm"] {
            let source = PathBuf::from(format!("{}{suffix}", db.display()));
            if source.exists() {
                fs::copy(&source, dir.join(format!("db.sqlite{suffix}"))).unwrap();
            }
        }
        let snapshot = dir.join("db.sqlite");
        let sessions = with_env_var("ZCODE_DB", &snapshot, scan_zcode);
        eprintln!("live ZCode sessions in snapshot: {}", sessions.len());
        assert!(!sessions.is_empty(), "expected real ZCode sessions here");
        let first = sessions.first().unwrap();
        eprintln!(
            "first session: '{}' ({}) turns={} project={}",
            first.title, first.id, first.turn_count, first.project_path
        );
        let archive = load_archive(first).unwrap();
        eprintln!(
            "loaded archive: {} messages from '{}'",
            archive.messages.len(),
            archive.title
        );
        assert_eq!(archive.source_provider, ChatProvider::Zcode);

        let archive_path = dir.join("live-zcode.agentswitch-chat.json");
        export_session(first, &archive_path).unwrap();
        let imported = with_env_var("ZCODE_DB", &snapshot, || {
            import_archive(&archive_path, None).unwrap()
        });
        assert_eq!(imported, snapshot, "native restore writes back to ZCODE_DB");
        let rescanned = with_env_var("ZCODE_DB", &snapshot, scan_zcode);
        assert!(
            rescanned.len() > sessions.len(),
            "imported copy appears as a new session"
        );
    }

    #[test]
    fn converts_claude_chat_into_codex_store() {
        let dir = temp_test_dir("convert-claude-to-codex");
        let home_a = dir.join("claude-home");
        let project_dir = home_a.join("projects").join("D--AI-Demo");
        fs::create_dir_all(&project_dir).unwrap();
        fs::write(
            project_dir.join("11111111-1111-1111-1111-111111111111.jsonl"),
            claude_fixture_lines("11111111-1111-1111-1111-111111111111"),
        )
        .unwrap();
        let codex_home = dir.join("codex-home");

        let source = with_env_var("CLAUDE_CONFIG_DIR", &home_a, || scan_claude().remove(0));
        let written = with_env_var("CODEX_HOME", &codex_home, || {
            convert_session(&source, ChatProvider::Codex)
        })
        .unwrap();
        assert!(written.exists(), "converted rollout exists");

        let converted = with_env_var("CODEX_HOME", &codex_home, scan_codex);
        assert_eq!(
            converted.len(),
            1,
            "converted chat is discoverable in Codex"
        );
        assert_eq!(converted[0].title, source.title);
        let archive = load_archive(&converted[0]).unwrap();
        let turns: Vec<_> = archive
            .messages
            .iter()
            .map(|m| (m.role.as_str(), m.text.as_str()))
            .collect();
        assert_eq!(
            turns,
            vec![("user", "hello claude"), ("assistant", "hi there")]
        );

        let raw = fs::read_to_string(&written).unwrap();
        for line in raw.lines() {
            let ev: serde_json::Value = serde_json::from_str(line).unwrap();
            assert!(
                matches!(
                    ev["type"].as_str(),
                    Some("session_meta") | Some("event_msg") | Some("response_item")
                ),
                "non-Codex schema leaked into converted rollout: {line}"
            );
        }
        let first: serde_json::Value = serde_json::from_str(raw.lines().next().unwrap()).unwrap();
        assert_eq!(first["type"], "session_meta");
        assert_eq!(first["payload"]["session_id"], first["payload"]["id"]);
        assert_eq!(first["payload"]["source"], "cli");
        assert_eq!(first["payload"]["originator"], "codex_cli_rs");
        assert_eq!(first["payload"]["thread_source"], "user");
        assert_eq!(first["payload"]["history_mode"], "legacy");

        let original = with_env_var("CLAUDE_CONFIG_DIR", &home_a, scan_claude);
        assert_eq!(original.len(), 1);
    }

    #[test]
    fn converts_opencode_db_chat_into_zcode_store() {
        let dir = temp_test_dir("convert-opencode-to-zcode");
        let schema = "CREATE TABLE session (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                slug TEXT NOT NULL,
                directory TEXT NOT NULL,
                title TEXT NOT NULL,
                version TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL
            );
            CREATE TABLE message (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                data TEXT NOT NULL
            );
            CREATE TABLE part (
                id TEXT PRIMARY KEY,
                message_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                data TEXT NOT NULL
            );";
        let src_db = dir.join("opencode.sqlite");
        let conn = Connection::open(&src_db).unwrap();
        conn.execute_batch(schema).unwrap();
        conn.execute(
            "INSERT INTO session (id, project_id, slug, directory, title, version, time_created, time_updated) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            rusqlite::params!["sess_oc", "proj", "sess_oc", "D:/work/x", "Cross store chat", "1", 1000_i64, 2000_i64],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?1,?2,?3,?4,?5)",
            rusqlite::params!["m1", "sess_oc", 1100_i64, 1100_i64, r#"{"role":"user"}"#],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES (?1,?2,?3,?4,?5,?6)",
            rusqlite::params!["p1", "m1", "sess_oc", 1100_i64, 1100_i64, r#"{"type":"text","text":"migrate me"}"#],
        )
        .unwrap();
        conn.close().unwrap();

        let dst_db = dir.join("zcode.sqlite");
        let conn = Connection::open(&dst_db).unwrap();
        conn.execute_batch(schema).unwrap();
        conn.close().unwrap();

        let source = with_env_var("OPENCODE_DB", &src_db, || scan_opencode().remove(0));
        with_env_var("ZCODE_DB", &dst_db, || {
            convert_session(&source, ChatProvider::Zcode)
        })
        .unwrap();

        let zcode_sessions = with_env_var("ZCODE_DB", &dst_db, scan_zcode);
        assert_eq!(zcode_sessions.len(), 1);
        assert_eq!(zcode_sessions[0].title, "Cross store chat");
        assert_eq!(zcode_sessions[0].project_path, "D:/work/x");
        let archive = load_archive(&zcode_sessions[0]).unwrap();
        assert_eq!(archive.messages.len(), 1);
        assert_eq!(archive.messages[0].text, "migrate me");

        assert_eq!(with_env_var("OPENCODE_DB", &src_db, scan_opencode).len(), 1);
    }

    #[test]
    fn conversion_rejects_antigravity_and_same_provider_targets() {
        let make = |provider| ChatSession {
            id: "s".into(),
            title: "t".into(),
            provider,
            project_path: String::new(),
            created_at: None,
            updated_at: String::new(),
            source_path: None,
            source_kind: ChatSourceKind::ImportedArchive,
            turn_count: 0,
            size_bytes: 0,
            imported: false,
            subagent: false,
            trash_manifest: None,
        };
        let error = convert_session(&make(ChatProvider::Antigravity), ChatProvider::Codex)
            .err()
            .unwrap();
        assert!(error.to_string().contains("encrypted"));
        let error = convert_session(&make(ChatProvider::Claude), ChatProvider::Antigravity)
            .err()
            .unwrap();
        assert!(error.to_string().contains("encrypted"));
        assert!(convert_session(&make(ChatProvider::Claude), ChatProvider::Claude).is_err());

        assert_eq!(
            convertible_targets(ChatProvider::Codex),
            vec![
                ChatProvider::Claude,
                ChatProvider::Kiro,
                ChatProvider::OpenCode,
                ChatProvider::Zcode
            ]
        );
        assert!(!conversion_targets().contains(&ChatProvider::Antigravity));
    }

    #[test]
    fn converted_archive_file_imports_into_chosen_project() {
        let dir = temp_test_dir("convert-archive-file");
        let home_a = dir.join("claude-home");
        let project_dir = home_a.join("projects").join("D--AI-Demo");
        fs::create_dir_all(&project_dir).unwrap();
        fs::write(
            project_dir.join("11111111-1111-1111-1111-111111111111.jsonl"),
            claude_fixture_lines("11111111-1111-1111-1111-111111111111"),
        )
        .unwrap();
        let source = with_env_var("CLAUDE_CONFIG_DIR", &home_a, || scan_claude().remove(0));

        let exported = dir.join("claude.demo.agentswitch-chat.json");
        export_session(&source, &exported).unwrap();

        let (converted, skipped) = convert_archive_file(&exported, ChatProvider::Codex).unwrap();
        assert_eq!(skipped, 0);
        assert!(converted
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains("codex"));
        let retagged: ChatArchive =
            serde_json::from_str(&fs::read_to_string(&converted).unwrap()).unwrap();
        assert_eq!(retagged.source_provider, ChatProvider::Codex);
        assert!(
            retagged.raw_events.is_empty(),
            "Claude event lines must not leak into a Codex archive"
        );
        assert_eq!(retagged.messages.len(), 2);

        let codex_home = dir.join("codex-home");
        let chosen_project = dir.join("chosen-project");
        fs::create_dir_all(&chosen_project).unwrap();
        with_env_var("CODEX_HOME", &codex_home, || {
            import_archive(&converted, Some(&chosen_project)).unwrap();
        });
        let found = with_env_var("CODEX_HOME", &codex_home, scan_codex);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].project_path, chosen_project.to_string_lossy());
        let archive = load_archive(&found[0]).unwrap();
        assert_eq!(archive.messages[0].text, "hello claude");
        assert_eq!(archive.messages[1].text, "hi there");
    }

    #[test]
    fn converted_zip_retagged_for_target_store() {
        let dir = temp_test_dir("convert-archive-zip");
        let make = |provider, id: &str, title: &str, with_raw: bool| ChatArchive {
            schema_version: ARCHIVE_VERSION,
            source_provider: provider,
            source_session_id: id.into(),
            title: title.into(),
            project_path: "D:/work/mix".into(),
            created_at: None,
            updated_at: None,
            messages: vec![ChatMessage {
                role: "user".into(),
                timestamp: None,
                text: format!("from {title}"),
            }],
            tool_calls: vec![],
            raw_events: if with_raw {
                vec![serde_json::json!({"type": "user", "message": {"role": "user"}})]
            } else {
                vec![]
            },
        };
        let a = make(ChatProvider::Claude, "c1", "Claude chat", true);
        let b = make(ChatProvider::Kiro, "k1", "Kiro chat", false);
        let zip_path = dir.join("mixed.zip");
        let file = File::create(&zip_path).unwrap();
        let mut zip = ZipWriter::new(file);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        for (name, archive) in [
            ("chats/a.agentswitch-chat.json", &a),
            ("chats/b.agentswitch-chat.json", &b),
        ] {
            zip.start_file(name, options).unwrap();
            zip.write_all(serde_json::to_string_pretty(archive).unwrap().as_bytes())
                .unwrap();
        }
        zip.finish().unwrap();

        let (converted, skipped) = convert_archive_file(&zip_path, ChatProvider::Codex).unwrap();
        assert_eq!(skipped, 0);
        assert!(converted.to_string_lossy().ends_with("-codex.zip"));

        let mut out = ZipArchive::new(File::open(&converted).unwrap()).unwrap();
        let mut seen = 0;
        for i in 0..out.len() {
            let mut entry = out.by_index(i).unwrap();
            let name = entry.name().to_string();
            if !name.ends_with(ARCHIVE_EXT) {
                continue;
            }
            let mut buf = String::new();
            entry.read_to_string(&mut buf).unwrap();
            let archive: ChatArchive = serde_json::from_str(&buf).unwrap();
            assert_eq!(archive.source_provider, ChatProvider::Codex);
            assert!(archive.raw_events.is_empty());
            seen += 1;
        }
        assert_eq!(seen, 2);

        let codex_home = dir.join("codex-home");
        let report =
            with_env_var("CODEX_HOME", &codex_home, || import_zip(&converted, None)).unwrap();
        assert_eq!(report.ok, 2);
        assert_eq!(report.failed, 0);
        assert_eq!(with_env_var("CODEX_HOME", &codex_home, scan_codex).len(), 2);
    }

    #[test]
    fn convert_archive_file_rejects_bad_input_and_antigravity() {
        let dir = temp_test_dir("convert-archive-guards");
        let make = |provider, title: &str| ChatArchive {
            schema_version: ARCHIVE_VERSION,
            source_provider: provider,
            source_session_id: "s".into(),
            title: title.into(),
            project_path: String::new(),
            created_at: None,
            updated_at: None,
            messages: vec![ChatMessage {
                role: "user".into(),
                timestamp: None,
                text: "x".into(),
            }],
            tool_calls: vec![],
            raw_events: vec![],
        };
        let antigravity = dir.join("agy.agentswitch-chat.json");
        fs::write(
            &antigravity,
            serde_json::to_string_pretty(&make(ChatProvider::Antigravity, "Encrypted origin"))
                .unwrap(),
        )
        .unwrap();
        let error = convert_archive_file(&antigravity, ChatProvider::Codex)
            .err()
            .unwrap();
        assert!(error.to_string().contains("encrypted"));

        let plain = dir.join("claude.agentswitch-chat.json");
        fs::write(
            &plain,
            serde_json::to_string_pretty(&make(ChatProvider::Claude, "Claude chat")).unwrap(),
        )
        .unwrap();
        let error = convert_archive_file(&plain, ChatProvider::Antigravity)
            .err()
            .unwrap();
        assert!(error.to_string().contains("encrypted"));

        let garbage = dir.join("garbage.agentswitch-chat.json");
        fs::write(&garbage, "not an archive").unwrap();
        assert!(convert_archive_file(&garbage, ChatProvider::Codex).is_err());
    }

    #[test]
    fn zcode_db_session_trashes_and_restores_with_identity() {
        let dir = temp_test_dir("zcode-db-trash");
        let db_path = dir.join("db.sqlite");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                slug TEXT NOT NULL,
                directory TEXT NOT NULL,
                title TEXT NOT NULL,
                version TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL
            );
            CREATE TABLE message (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                data TEXT NOT NULL
            );
            CREATE TABLE part (
                id TEXT PRIMARY KEY,
                message_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                data TEXT NOT NULL
            );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session (id, project_id, slug, directory, title, version, time_created, time_updated) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            rusqlite::params!["sess_abc", "proj", "sess_abc", "D:/work/trash", "Trashable chat", "1", 1780000000000_i64, 1780001000000_i64],
        )
        .unwrap();
        for (mid, pid, role, text) in [
            ("m1", "p1", "user", "first question"),
            ("m2", "p2", "assistant", "first answer"),
        ] {
            conn.execute(
                "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?1,?2,?3,?4,?5)",
                rusqlite::params![mid, "sess_abc", 1780000100000_i64, 1780000100000_i64, format!(r#"{{"role":"{role}"}}"#)],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES (?1,?2,?3,?4,?5,?6)",
                rusqlite::params![pid, mid, "sess_abc", 1780000100000_i64, 1780000100000_i64, format!(r#"{{"type":"text","text":"{text}"}}"#)],
            )
            .unwrap();
        }
        conn.close().unwrap();

        let data = dir.join("data");
        with_env_vars(
            &[("AGENT_SWITCH_DATA_DIR", &data), ("ZCODE_DB", &db_path)],
            || {
                let held = Connection::open(&db_path).unwrap();
                let _count: i64 = held
                    .query_row("SELECT COUNT(*) FROM session", [], |row| row.get(0))
                    .unwrap();

                let session = scan_zcode().remove(0);
                soft_delete(&session).unwrap();

                assert!(scan_zcode().is_empty());
                let trash = scan_trash(None);
                assert_eq!(trash.len(), 1);
                assert_eq!(trash[0].provider, ChatProvider::Zcode);
                assert_eq!(trash[0].id, "sess_abc");
                assert_eq!(trash[0].title, "Trashable chat");
                let archived = load_archive(&trash[0]).unwrap();
                assert_eq!(archived.messages.len(), 2);
                assert_eq!(archived.messages[1].text, "first answer");
                drop(held);

                restore_from_trash(&trash[0]).unwrap();
                let restored = scan_zcode();
                assert_eq!(restored.len(), 1);
                assert_eq!(restored[0].id, "sess_abc");
                assert_eq!(restored[0].title, "Trashable chat");
                assert_eq!(restored[0].project_path, "D:/work/trash");
                let back = load_archive(&restored[0]).unwrap();
                assert_eq!(back.messages.len(), 2);
                assert_eq!(back.messages[0].text, "first question");
                assert!(scan_trash(None).is_empty());
            },
        );
    }

    #[test]
    fn opencode_legacy_db_session_trashes_cleanly() {
        let dir = temp_test_dir("opencode-legacy-trash");
        let db_path = dir.join("legacy.sqlite");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                slug TEXT NOT NULL,
                directory TEXT NOT NULL,
                title TEXT NOT NULL,
                version TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL
            );
            CREATE TABLE session_message (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                data TEXT NOT NULL
            );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session (id, project_id, slug, directory, title, version, time_created, time_updated) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            rusqlite::params!["sess_leg", "proj", "sess_leg", "D:/work/old", "Legacy chat", "1", 100_i64, 200_i64],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_message (id, session_id, time_created, data) VALUES (?1,?2,?3,?4)",
            rusqlite::params![
                "sm1",
                "sess_leg",
                150_i64,
                r#"{"role":"user","content":"old message"}"#
            ],
        )
        .unwrap();
        conn.close().unwrap();

        let data = dir.join("data");
        with_env_vars(
            &[("AGENT_SWITCH_DATA_DIR", &data), ("OPENCODE_DB", &db_path)],
            || {
                let session = scan_opencode().remove(0);
                soft_delete(&session).unwrap();
                assert!(scan_opencode().is_empty());
                let trash = scan_trash(None);
                assert_eq!(trash.len(), 1);
                assert_eq!(trash[0].title, "Legacy chat");
                let archived = load_archive(&trash[0]).unwrap();
                assert_eq!(archived.messages.len(), 1);
                assert_eq!(archived.messages[0].text, "old message");
            },
        );
    }

    fn codex_threads_db_fixture(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE threads (
                id TEXT PRIMARY KEY,
                rollout_path TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                source TEXT NOT NULL,
                model_provider TEXT NOT NULL,
                cwd TEXT NOT NULL,
                title TEXT NOT NULL,
                sandbox_policy TEXT NOT NULL,
                approval_mode TEXT NOT NULL,
                tokens_used INTEGER NOT NULL DEFAULT 0,
                has_user_event INTEGER NOT NULL DEFAULT 0,
                archived INTEGER NOT NULL DEFAULT 0,
                archived_at INTEGER,
                git_sha TEXT,
                git_branch TEXT,
                git_origin_url TEXT,
                cli_version TEXT NOT NULL DEFAULT '',
                first_user_message TEXT NOT NULL DEFAULT '',
                agent_nickname TEXT,
                agent_role TEXT,
                memory_mode TEXT NOT NULL DEFAULT 'enabled',
                model TEXT,
                reasoning_effort TEXT,
                agent_path TEXT,
                created_at_ms INTEGER,
                updated_at_ms INTEGER,
                thread_source TEXT,
                preview TEXT NOT NULL DEFAULT '',
                recency_at INTEGER NOT NULL DEFAULT 0,
                recency_at_ms INTEGER NOT NULL DEFAULT 0,
                history_mode TEXT NOT NULL DEFAULT 'legacy',
                name TEXT,
                is_pinned INTEGER NOT NULL DEFAULT 0,
                thread_section_id TEXT,
                section_position INTEGER,
                section_entered_at_ms INTEGER,
                project_id TEXT
            );",
        )
        .unwrap();
        conn.close().unwrap();
    }

    #[test]
    fn failed_codex_registration_keeps_trash_manifest() {
        let dir = temp_test_dir("codex-failed-register");
        let codex_home = dir.join("codex-home");
        fs::create_dir_all(&codex_home).unwrap();
        codex_threads_db_fixture(&codex_home.join("state_5.sqlite"));
        let data = dir.join("data");

        with_env_vars(
            &[
                ("CODEX_HOME", &codex_home),
                ("AGENT_SWITCH_DATA_DIR", &data),
            ],
            || {
                let sessions_dir = codex_home.join("sessions");
                fs::create_dir_all(&sessions_dir).unwrap();
                write_sample_jsonl(&sessions_dir, "rollout-x", dir.to_string_lossy().as_ref());
                let session = scan_codex().remove(0);

                soft_delete(&session).unwrap();
                let trash = scan_trash(None);
                assert_eq!(trash.len(), 1);
                let manifest_path = trash[0].trash_manifest.clone().unwrap();

                fs::write(codex_home.join("state_5.sqlite"), b"not a database").unwrap();
                assert!(
                    restore_from_trash(&trash[0]).is_err(),
                    "restore must surface the registration failure"
                );
                assert!(manifest_path.exists(), "manifest survives a half-restore");
            },
        );
    }

    #[test]
    fn codex_thread_registered_in_state_database_across_trash_cycle() {
        let dir = temp_test_dir("codex-state-db");
        let codex_home = dir.join("codex-home");
        fs::create_dir_all(&codex_home).unwrap();
        codex_threads_db_fixture(&codex_home.join("state_5.sqlite"));
        let home_a = dir.join("claude-home");
        let project_dir = home_a.join("projects").join("D--AI-Demo");
        fs::create_dir_all(&project_dir).unwrap();
        fs::write(
            project_dir.join("11111111-1111-1111-1111-111111111111.jsonl"),
            claude_fixture_lines("11111111-1111-1111-1111-111111111111"),
        )
        .unwrap();
        let data = dir.join("data");

        let converted_path = with_env_vars(
            &[("CLAUDE_CONFIG_DIR", &home_a), ("CODEX_HOME", &codex_home)],
            || {
                let session = scan_claude().remove(0);
                convert_session(&session, ChatProvider::Codex).unwrap()
            },
        );

        with_env_vars(
            &[
                ("AGENT_SWITCH_DATA_DIR", &data),
                ("CODEX_HOME", &codex_home),
            ],
            || {
                let db_path = codex_home.join("state_5.sqlite");
                let thread_rows = || {
                    let conn = Connection::open(&db_path).unwrap();
                    let mut stmt = conn
                        .prepare("SELECT rollout_path, title, source, cwd FROM threads")
                        .unwrap();
                    let rows: Vec<(String, String, String, String)> = stmt
                        .query_map([], |row| {
                            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                        })
                        .unwrap()
                        .filter_map(|r| r.ok())
                        .collect();
                    drop(stmt);
                    conn.close().unwrap();
                    rows
                };

                let rows = thread_rows();
                assert_eq!(rows.len(), 1, "converted rollout is indexed");
                assert_eq!(
                    PathBuf::from(&rows[0].0),
                    converted_path,
                    "row points at the written rollout"
                );
                assert_eq!(rows[0].2, "cli");

                let session = scan_codex().remove(0);
                soft_delete(&session).unwrap();
                assert!(thread_rows().is_empty(), "trashed chat leaves /resume");
                let trash = scan_trash(None);
                assert_eq!(trash.len(), 1);
                restore_from_trash(&trash[0]).unwrap();
                assert!(converted_path.exists());
                let rows = thread_rows();
                assert_eq!(rows.len(), 1, "restored chat is listed again");
                assert_eq!(PathBuf::from(&rows[0].0), converted_path);
                let conn = Connection::open(&db_path).unwrap();
                let (secs, ms): (i64, i64) = conn
                    .query_row("SELECT updated_at, updated_at_ms FROM threads", [], |row| {
                        Ok((row.get(0)?, row.get(1)?))
                    })
                    .unwrap();
                drop(conn);
                assert!(
                    ms > secs,
                    "updated_at_ms ({ms}) must exceed updated_at ({secs})"
                );
            },
        );
    }

    #[test]
    fn restoring_imported_codex_archive_skips_state_database() {
        let dir = temp_test_dir("codex-imported-restore");
        let codex_home = dir.join("codex-home");
        fs::create_dir_all(&codex_home).unwrap();
        codex_threads_db_fixture(&codex_home.join("state_5.sqlite"));
        let data = dir.join("data");

        let archive = ChatArchive {
            schema_version: 1,
            source_provider: ChatProvider::Codex,
            source_session_id: "imported-session".into(),
            title: "Imported chat".into(),
            project_path: dir.to_string_lossy().to_string(),
            created_at: Some("2026-05-01T00:00:00Z".into()),
            updated_at: Some("2026-05-01T00:01:00Z".into()),
            messages: vec![crate::chat::ChatMessage {
                role: "user".into(),
                timestamp: None,
                text: "hello imported".into(),
            }],
            tool_calls: vec![],
            raw_events: vec![],
        };
        let imports = with_env_var("AGENT_SWITCH_DATA_DIR", &data, imports_dir);
        fs::create_dir_all(&imports).unwrap();
        let archive_path = imports.join("codex-imported.agentswitch-chat.json");
        fs::write(
            &archive_path,
            serde_json::to_string_pretty(&archive).unwrap(),
        )
        .unwrap();

        with_env_vars(
            &[
                ("CODEX_HOME", &codex_home),
                ("AGENT_SWITCH_DATA_DIR", &data),
            ],
            || {
                let session = scan_imported().remove(0);
                soft_delete(&session).unwrap();
                let trash = scan_trash(None);
                assert_eq!(trash.len(), 1);

                let db_path = codex_home.join("state_5.sqlite");
                let thread_count = || {
                    let conn = Connection::open(&db_path).unwrap();
                    let count: i64 = conn
                        .query_row("SELECT COUNT(*) FROM threads", [], |r| r.get(0))
                        .unwrap();
                    drop(conn);
                    count
                };
                assert_eq!(thread_count(), 0);

                restore_from_trash(&trash[0]).unwrap();

                assert_eq!(
                    thread_count(),
                    0,
                    "imported archive restore must not touch Codex's threads table"
                );
                assert!(archive_path.exists(), "archive file is restored");
            },
        );
    }

    #[test]
    fn zcode_conversion_matches_current_schema() {
        let dir = temp_test_dir("zcode-current-schema");
        let db_path = dir.join("db.sqlite");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_migration (id TEXT PRIMARY KEY);
             CREATE TABLE projects (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                metadata TEXT NOT NULL DEFAULT '{}',
                position INTEGER NOT NULL,
                created_at_ms INTEGER NOT NULL,
                updated_at_ms INTEGER NOT NULL
            );
            CREATE TABLE project_roots (
                project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                position INTEGER NOT NULL,
                path TEXT NOT NULL,
                PRIMARY KEY (project_id, position)
            );
            CREATE TABLE session (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                workspace_id TEXT,
                parent_id TEXT,
                slug TEXT NOT NULL,
                directory TEXT NOT NULL,
                path TEXT,
                title TEXT NOT NULL,
                version TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                task_type TEXT NOT NULL DEFAULT 'interactive',
                title_source TEXT NOT NULL DEFAULT 'first_input'
            );
            CREATE TABLE message (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                data TEXT NOT NULL
            );
            CREATE TABLE part (
                id TEXT PRIMARY KEY,
                message_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                data TEXT NOT NULL
            );
            INSERT INTO session (id, project_id, slug, directory, title, version, time_created, time_updated)
                VALUES ('sess_real', 'proj_d-work-app', 'sess_real', 'D:/work/app', 'Native chat', '0.16.3', 1000, 2000);",
        )
        .unwrap();
        conn.close().unwrap();

        let home_a = dir.join("claude-home");
        let project_dir = home_a.join("projects").join("D--Work-App");
        fs::create_dir_all(&project_dir).unwrap();
        fs::write(
            project_dir.join("11111111-1111-1111-1111-111111111111.jsonl"),
            claude_fixture_lines("11111111-1111-1111-1111-111111111111"),
        )
        .unwrap();

        with_env_vars(
            &[("CLAUDE_CONFIG_DIR", &home_a), ("ZCODE_DB", &db_path)],
            || {
                let source = scan_claude().remove(0);
                convert_session(&source, ChatProvider::Zcode).unwrap();

                let conn = Connection::open(&db_path).unwrap();
                let row = conn
                    .query_row(
                        "SELECT project_id, version, path, task_type, title_source FROM session WHERE id != 'sess_real'",
                        [],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, Option<String>>(2)?,
                                row.get::<_, String>(3)?,
                                row.get::<_, String>(4)?,
                            ))
                        },
                    )
                    .unwrap();
                assert_eq!(row.0, "proj_d--ai-demo");
                assert_eq!(row.1, "0.16.3", "adopts the store's app version");
                assert_eq!(row.2.as_deref(), Some(r"D:\AI\Demo"));
                assert_eq!(row.3, "interactive");
                assert_eq!(row.4, "generated");
                let projects: i64 = conn
                    .query_row("SELECT COUNT(*) FROM projects", [], |r| r.get(0))
                    .unwrap();
                assert_eq!(projects, 1, "projects row backs the project id");
                let roots: i64 = conn
                    .query_row("SELECT COUNT(*) FROM project_roots", [], |r| r.get(0))
                    .unwrap();
                assert_eq!(roots, 1);
                let messages: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM message WHERE session_id != 'sess_real'",
                        [],
                        |r| r.get(0),
                    )
                    .unwrap();
                assert_eq!(messages, 2);
                conn.close().unwrap();
            },
        );
    }

    #[test]
    fn kiro_restore_events_carry_message_ids_and_timestamps() {
        let dir = temp_test_dir("kiro-restore");
        let home = dir.join(".kiro");
        let archive = ChatArchive {
            schema_version: ARCHIVE_VERSION,
            source_provider: ChatProvider::Codex,
            source_session_id: "src".into(),
            title: "Kiro bound chat".into(),
            project_path: r"D:\AI\demo".into(),
            created_at: None,
            updated_at: None,
            messages: vec![ChatMessage {
                role: "user".into(),
                timestamp: Some("2026-05-01T00:00:00Z".into()),
                text: "migrate to kiro".into(),
            }],
            tool_calls: vec![],
            raw_events: vec![],
        };
        with_env_var("KIRO_HOME", &home, || {
            restore_kiro_native(&archive, None).unwrap();
            let root = home.join("sessions").join("cli");
            let jsonl_path = fs::read_dir(&root)
                .unwrap()
                .flatten()
                .map(|entry| entry.path())
                .find(|path| path.extension().and_then(|e| e.to_str()) == Some("jsonl"))
                .unwrap();
            let raw = fs::read_to_string(&jsonl_path).unwrap();
            let event: serde_json::Value =
                serde_json::from_str(raw.lines().next().unwrap()).unwrap();
            assert_eq!(event["version"], "v1");
            assert_eq!(event["kind"], "Prompt");
            assert!(
                event["data"]["message_id"].is_string(),
                "real Kiro events carry a message_id"
            );
            let meta_timestamp = event["data"]["meta"]["timestamp"]
                .as_i64()
                .expect("meta.timestamp unix seconds present");
            assert_eq!(meta_timestamp, 1777593600);
            let sessions = scan_kiro();
            assert_eq!(sessions.len(), 1);
            let archive = load_archive(&sessions[0]).unwrap();
            assert_eq!(
                archive.messages[0].timestamp.as_deref(),
                Some("2026-05-01T00:00:00Z")
            );
        });
    }

    fn provider_env(dir: &std::path::Path, provider: ChatProvider) -> (&'static str, PathBuf) {
        match provider {
            ChatProvider::Claude => ("CLAUDE_CONFIG_DIR", dir.join("claude-home")),
            ChatProvider::Codex => ("CODEX_HOME", dir.join("codex-home")),
            ChatProvider::Kiro => ("KIRO_HOME", dir.join("kiro-home")),
            ChatProvider::OpenCode => ("OPENCODE_DB", dir.join("opencode.sqlite")),
            ChatProvider::Zcode => ("ZCODE_DB", dir.join("zcode.sqlite")),
            ChatProvider::Antigravity => unreachable!("excluded from conversions"),
        }
    }

    const SESSION_SCHEMA_MINIMAL: &str = "
        CREATE TABLE session (
            id TEXT PRIMARY KEY,
            project_id TEXT NOT NULL,
            slug TEXT NOT NULL,
            directory TEXT NOT NULL,
            title TEXT NOT NULL,
            version TEXT NOT NULL,
            time_created INTEGER NOT NULL,
            time_updated INTEGER NOT NULL
        );
        CREATE TABLE message (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            time_created INTEGER NOT NULL,
            time_updated INTEGER NOT NULL,
            data TEXT NOT NULL
        );
        CREATE TABLE part (
            id TEXT PRIMARY KEY,
            message_id TEXT NOT NULL,
            session_id TEXT NOT NULL,
            time_created INTEGER NOT NULL,
            time_updated INTEGER NOT NULL,
            data TEXT NOT NULL
        );";

    fn create_empty_session_db(path: &std::path::Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(SESSION_SCHEMA_MINIMAL).unwrap();
        conn.close().unwrap();
    }

    fn seed_sqlite_store(path: &std::path::Path, first: &str, second: &str) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(SESSION_SCHEMA_MINIMAL).unwrap();
        let batch = "
            INSERT INTO session (id, project_id, slug, directory, title, version, time_created, time_updated)
                VALUES ('sess_mx', 'proj_mx', 'sess_mx', 'D:/work/mx', 'Matrix chat', '1', 1000, 2000);
            INSERT INTO message (id, session_id, time_created, time_updated, data)
                VALUES ('m1', 'sess_mx', 1100, 1100, '{\"role\":\"user\"}');
            INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
                VALUES ('p1', 'm1', 'sess_mx', 1100, 1100, '{\"type\":\"text\",\"text\":\"QUESTION\"}');
            INSERT INTO message (id, session_id, time_created, time_updated, data)
                VALUES ('m2', 'sess_mx', 1200, 1200, '{\"role\":\"assistant\"}');
            INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
                VALUES ('p2', 'm2', 'sess_mx', 1200, 1200, '{\"type\":\"text\",\"text\":\"ANSWER\"}');"
            .replace("QUESTION", first)
            .replace("ANSWER", second);
        conn.execute_batch(&batch).unwrap();
        conn.close().unwrap();
    }

    fn build_source_store(
        dir: &std::path::Path,
        provider: ChatProvider,
    ) -> (&'static str, &'static str) {
        match provider {
            ChatProvider::Claude => {
                let home = dir.join("claude-home");
                let project_dir = home.join("projects").join("D--AI-Demo");
                fs::create_dir_all(&project_dir).unwrap();
                fs::write(
                    project_dir.join("11111111-1111-1111-1111-111111111111.jsonl"),
                    claude_fixture_lines("11111111-1111-1111-1111-111111111111"),
                )
                .unwrap();
                ("hello claude", "hi there")
            }
            ChatProvider::Codex => {
                let sessions_dir = dir
                    .join("codex-home")
                    .join("sessions")
                    .join("2026")
                    .join("05")
                    .join("01");
                fs::create_dir_all(&sessions_dir).unwrap();
                fs::write(
                    sessions_dir.join("rollout-2026-05-01T00-00-00-33333333-3333-3333-3333-333333333333.jsonl"),
                    r#"{"timestamp":"2026-05-01T00:00:00Z","type":"session_meta","payload":{"id":"33333333-3333-3333-3333-333333333333","cwd":"D:/work/mx"}}"#.to_string()
                        + "\n"
                        + r#"{"timestamp":"2026-05-01T00:01:00Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"codex question"}]}}"#
                        + "\n"
                        + r#"{"timestamp":"2026-05-01T00:02:00Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"codex answer"}]}}"#,
                )
                .unwrap();
                ("codex question", "codex answer")
            }
            ChatProvider::Kiro => {
                let cli = dir.join("kiro-home").join("sessions").join("cli");
                fs::create_dir_all(&cli).unwrap();
                let id = "44444444-4444-4444-4444-444444444444";
                fs::write(
                    cli.join(format!("{id}.json")),
                    format!(
                        r#"{{"session_id":"{id}","cwd":"D:\\AI","created_at":"2026-05-01T00:00:00Z","updated_at":"2026-05-01T00:01:00Z","title":"Kiro matrix","session_state":{{}}}}"#
                    ),
                )
                .unwrap();
                fs::write(
                    cli.join(format!("{id}.jsonl")),
                    r#"{"version":"v1","kind":"Prompt","data":{"message_id":"u1","content":[{"kind":"text","data":"kiro question"}]}}"#.to_string()
                        + "\n"
                        + r#"{"version":"v1","kind":"AssistantMessage","data":{"message_id":"a1","content":[{"kind":"text","data":"kiro answer"}]}}"#,
                )
                .unwrap();
                ("kiro question", "kiro answer")
            }
            ChatProvider::OpenCode => {
                seed_sqlite_store(
                    &dir.join("opencode.sqlite"),
                    "opencode question",
                    "opencode answer",
                );
                ("opencode question", "opencode answer")
            }
            ChatProvider::Zcode => {
                seed_sqlite_store(&dir.join("zcode.sqlite"), "zcode question", "zcode answer");
                ("zcode question", "zcode answer")
            }
            ChatProvider::Antigravity => unreachable!("excluded from conversions"),
        }
    }

    fn scan_provider(provider: ChatProvider, _project: &str) -> Vec<ChatSession> {
        match provider {
            ChatProvider::Claude => scan_claude(),
            ChatProvider::Codex => scan_codex(),
            ChatProvider::Kiro => scan_kiro(),
            ChatProvider::OpenCode => scan_opencode(),
            ChatProvider::Zcode => scan_zcode(),
            ChatProvider::Antigravity => unreachable!("excluded from conversions"),
        }
    }

    #[test]
    fn every_provider_pair_converts_and_lands_discoverable() {
        let providers = [
            ChatProvider::Claude,
            ChatProvider::Codex,
            ChatProvider::Kiro,
            ChatProvider::OpenCode,
            ChatProvider::Zcode,
        ];
        let mut combos = 0;
        for source in providers {
            for target in providers {
                if source == target {
                    continue;
                }
                combos += 1;
                let dir = temp_test_dir(&format!("matrix-{}-to-{}", source.id(), target.id()));
                let (first, second) = build_source_store(&dir, source);
                let (source_var, source_path) = provider_env(&dir, source);
                let (target_var, target_path) = provider_env(&dir, target);
                if matches!(target, ChatProvider::OpenCode | ChatProvider::Zcode) {
                    create_empty_session_db(&target_path);
                }

                with_env_vars(
                    &[(source_var, &source_path), (target_var, &target_path)],
                    || {
                        let source_project = if source == ChatProvider::Kiro {
                            r"D:\AI"
                        } else {
                            ""
                        };
                        let session = scan_provider(source, source_project).remove(0);
                        let project = session.project_path.clone();
                        convert_session(&session, target).unwrap();

                        let converted = scan_provider(target, &project);
                        assert_eq!(
                            converted.len(),
                            1,
                            "{source:?} -> {target:?}: converted chat is discoverable"
                        );
                        let archive = load_archive(&converted[0]).unwrap();
                        let texts: Vec<_> = archive
                            .messages
                            .iter()
                            .map(|message| (message.role.as_str(), message.text.as_str()))
                            .collect();
                        assert_eq!(
                            texts,
                            vec![("user", first), ("assistant", second)],
                            "{source:?} -> {target:?}: turns survive conversion"
                        );
                    },
                );
            }
        }
        assert_eq!(combos, 20, "5 providers x 4 targets, Antigravity excluded");
    }
}
