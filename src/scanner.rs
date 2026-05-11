use crate::types::*;
use std::path::{Path, PathBuf};

pub fn scan_provider(id: ProviderId, root: &Path) -> Vec<ConfigItem> {
    match id {
        ProviderId::Claude => scan_claude(root),
        ProviderId::Codex => scan_codex(root),
        ProviderId::Gemini => scan_gemini(root),
        ProviderId::Kiro => scan_kiro(root),
        ProviderId::OpenCode => scan_opencode(root),
    }
}

pub fn provider_exists(id: ProviderId, root: &Path) -> bool {
    match id {
        ProviderId::Claude => root.join(".claude").is_dir() || has_cmd("claude"),
        ProviderId::Codex => root.join(".codex").is_dir() || root.join(".agents").is_dir() || has_cmd("codex"),
        ProviderId::Gemini => root.join(".gemini").is_dir() || has_cmd("gemini"),
        ProviderId::Kiro => root.join(".kiro").is_dir() || has_cmd("kiro") || has_cmd("kiro-cli"),
        ProviderId::OpenCode => root.join(".opencode").is_dir() || has_cmd("opencode"),
    }
}

fn has_cmd(name: &str) -> bool {
    std::process::Command::new(if cfg!(windows) { "where" } else { "which" })
        .arg(name).output().map(|o| o.status.success()).unwrap_or(false)
}

fn collect_md(dir: &Path, kind: ItemKind, provider: ProviderId) -> Vec<ConfigItem> {
    let mut out = vec![];
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
            let is_md = name.ends_with(".md") || name.ends_with(".md.disabled");
            if p.is_file() && is_md {
                out.push(ConfigItem::new(name, kind, p, provider));
            }
        }
    }
    out
}

fn collect_subdirs(dir: &Path, kind: ItemKind, provider: ProviderId) -> Vec<ConfigItem> {
    let mut out = vec![];
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
                out.push(ConfigItem::new(name, kind, p, provider));
            }
        }
    }
    out
}

fn check_file(path: PathBuf, kind: ItemKind, provider: ProviderId) -> Vec<ConfigItem> {
    let mut out = vec![];
    if path.exists() {
        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        out.push(ConfigItem::new(name, kind, path.clone(), provider));
    }
    // also check .disabled variant
    let dis = PathBuf::from(format!("{}.disabled", path.display()));
    if dis.exists() {
        let name = dis.file_name().unwrap_or_default().to_string_lossy().to_string();
        out.push(ConfigItem::new(name, ItemKind::InstructionFile, dis, provider));
    }
    out
}

fn scan_json_keys(path: &Path, key: &str, kind: ItemKind, provider: ProviderId) -> Vec<ConfigItem> {
    let mut out = vec![];
    if let Ok(text) = std::fs::read_to_string(path) {
        if let Ok(doc) = serde_json::from_str::<serde_json::Value>(&text) {
            for (check_key, state) in [(key, ItemState::Enabled), (&format!("_disabled_{}", key), ItemState::Disabled)] {
                if let Some(obj) = doc.get(check_key).and_then(|v| v.as_object()) {
                    for name in obj.keys() {
                        let mut item = ConfigItem::new(name.clone(), kind, path.to_owned(), provider);
                        item.state = state;
                        item.editable = false;
                        out.push(item);
                    }
                }
            }
        }
    }
    out
}

fn scan_claude(root: &Path) -> Vec<ConfigItem> {
    let d = root.join(".claude");
    let mut items = vec![];
    items.extend(check_file(root.join("CLAUDE.md"), ItemKind::InstructionFile, ProviderId::Claude));
    items.extend(collect_subdirs(&d.join("skills"), ItemKind::Skill, ProviderId::Claude));
    items.extend(collect_md(&d.join("rules"), ItemKind::Rule, ProviderId::Claude));
    let settings = d.join("settings.json");
    items.extend(scan_json_keys(&settings, "hooks", ItemKind::Hook, ProviderId::Claude));
    items.extend(scan_json_keys(&settings, "mcpServers", ItemKind::Mcp, ProviderId::Claude));
    items
}

fn scan_codex(root: &Path) -> Vec<ConfigItem> {
    let mut items = vec![];
    items.extend(check_file(root.join("AGENTS.md"), ItemKind::InstructionFile, ProviderId::Codex));
    for base in [".agents", ".codex"] {
        items.extend(collect_subdirs(&root.join(base).join("skills"), ItemKind::Skill, ProviderId::Codex));
    }
    let mcp = root.join(".mcp.json");
    items.extend(scan_json_keys(&mcp, "mcpServers", ItemKind::Mcp, ProviderId::Codex));
    // hooks.json
    let hooks = root.join(".codex").join("hooks.json");
    if hooks.exists() || hooks.with_extension("json.disabled").exists() {
        items.push(ConfigItem::new("hooks.json", ItemKind::Hook, hooks, ProviderId::Codex));
    }
    items
}

fn scan_gemini(root: &Path) -> Vec<ConfigItem> {
    let d = root.join(".gemini");
    let mut items = vec![];
    items.extend(check_file(root.join("GEMINI.md"), ItemKind::InstructionFile, ProviderId::Gemini));
    items.extend(check_file(root.join("AGENTS.md"), ItemKind::InstructionFile, ProviderId::Gemini));
    items.extend(collect_subdirs(&d.join("skills"), ItemKind::Skill, ProviderId::Gemini));
    items.extend(collect_md(&d.join("rules"), ItemKind::Rule, ProviderId::Gemini));
    let hooks = d.join("hooks").join("hooks.json");
    if hooks.exists() {
        items.push(ConfigItem::new("hooks.json", ItemKind::Hook, hooks.clone(), ProviderId::Gemini));
    }
    let hd = hooks.with_extension("json.disabled");
    if hd.exists() {
        items.push(ConfigItem::new("hooks.json.disabled", ItemKind::Hook, hd, ProviderId::Gemini));
    }
    let settings = d.join("settings.json");
    items.extend(scan_json_keys(&settings, "mcpServers", ItemKind::Mcp, ProviderId::Gemini));
    items
}

fn scan_kiro(root: &Path) -> Vec<ConfigItem> {
    let d = root.join(".kiro");
    let mut items = vec![];
    items.extend(collect_md(&d.join("steering"), ItemKind::SteeringRule, ProviderId::Kiro));
    items.extend(collect_subdirs(&d.join("specs"), ItemKind::Spec, ProviderId::Kiro));
    let mcp = d.join("settings").join("mcp.json");
    items.extend(scan_json_keys(&mcp, "mcpServers", ItemKind::Mcp, ProviderId::Kiro));
    items
}

fn scan_opencode(root: &Path) -> Vec<ConfigItem> {
    let mut items = vec![];
    items.extend(check_file(root.join("AGENTS.md"), ItemKind::InstructionFile, ProviderId::OpenCode));
    items.extend(collect_subdirs(&root.join(".opencode").join("skills"), ItemKind::Skill, ProviderId::OpenCode));
    let cfg = root.join("opencode.json");
    items.extend(scan_json_keys(&cfg, "agent", ItemKind::Agent, ProviderId::OpenCode));
    items.extend(scan_json_keys(&cfg, "mcp", ItemKind::Mcp, ProviderId::OpenCode));
    items
}
