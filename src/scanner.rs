use crate::types::*;
use std::path::{Path, PathBuf};

pub fn scan_provider(id: ProviderId, root: &Path, scope: Scope) -> Vec<ConfigItem> {
    match id {
        ProviderId::Claude => scan_claude(root, scope),
        ProviderId::Codex => scan_codex(root, scope),
        ProviderId::Gemini => scan_gemini(root, scope),
        ProviderId::Kiro => scan_kiro(root, scope),
        ProviderId::OpenCode => scan_opencode(root, scope),
    }
}

pub fn provider_exists(id: ProviderId, root: &Path, scope: Scope) -> bool {
    match (id, scope) {
        (ProviderId::Claude, _) => provider_dir(id, root, scope).is_dir() || has_cmd("claude"),
        (ProviderId::Codex, Scope::Project) => {
            root.join(".codex").is_dir()
                || root.join(".agents").is_dir()
                || root.join(".mcp.json").is_file()
                || has_cmd("codex")
        }
        (ProviderId::Codex, Scope::Global) => {
            provider_dir(id, root, scope).is_dir() || has_cmd("codex")
        }
        (ProviderId::Gemini, _) => provider_dir(id, root, scope).is_dir() || has_cmd("gemini"),
        (ProviderId::Kiro, _) => {
            provider_dir(id, root, scope).is_dir() || has_cmd("kiro") || has_cmd("kiro-cli")
        }
        (ProviderId::OpenCode, _) => provider_dir(id, root, scope).is_dir() || has_cmd("opencode"),
    }
}

pub fn provider_dir(id: ProviderId, root: &Path, scope: Scope) -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    match (id, scope) {
        (ProviderId::Claude, Scope::Project) => root.join(".claude"),
        (ProviderId::Claude, Scope::Global) => home.join(".claude"),
        (ProviderId::Codex, Scope::Project) => root.join(".codex"),
        (ProviderId::Codex, Scope::Global) => home.join(".codex"),
        (ProviderId::Gemini, Scope::Project) => root.join(".gemini"),
        (ProviderId::Gemini, Scope::Global) => home.join(".gemini"),
        (ProviderId::Kiro, Scope::Project) => root.join(".kiro"),
        (ProviderId::Kiro, Scope::Global) => home.join(".kiro"),
        (ProviderId::OpenCode, Scope::Project) => root.join(".opencode"),
        (ProviderId::OpenCode, Scope::Global) => home.join(".config").join("opencode"),
    }
}

fn has_cmd(name: &str) -> bool {
    let mut cmd = std::process::Command::new(if cfg!(windows) { "where" } else { "which" });
    cmd.arg(name);
    hide_cmd_window(&mut cmd);
    cmd.output().map(|o| o.status.success()).unwrap_or(false)
}

#[cfg(windows)]
fn hide_cmd_window(cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x08000000);
}

#[cfg(not(windows))]
fn hide_cmd_window(_: &mut std::process::Command) {}

fn collect_md(dir: &Path, kind: ItemKind, provider: ProviderId) -> Vec<ConfigItem> {
    let mut out = vec![];
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            let name = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            if p.is_file() && (name.ends_with(".md") || name.ends_with(".md.disabled")) {
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
                let name = p
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                out.push(ConfigItem::new(name, kind, p, provider));
            }
        }
    }
    out
}

fn check_file(path: PathBuf, kind: ItemKind, provider: ProviderId) -> Vec<ConfigItem> {
    let mut out = vec![];
    if path.exists() {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        out.push(ConfigItem::new(name, kind, path.clone(), provider));
    }
    let dis = PathBuf::from(format!("{}.disabled", path.display()));
    if dis.exists() {
        let name = dis
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        out.push(ConfigItem::new(
            name,
            ItemKind::InstructionFile,
            dis,
            provider,
        ));
    }
    out
}

fn scan_json_keys(path: &Path, key: &str, kind: ItemKind, provider: ProviderId) -> Vec<ConfigItem> {
    let mut out = vec![];
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        _ => return out,
    };
    let doc: serde_json::Value = match serde_json::from_str(&text) {
        Ok(d) => d,
        _ => return out,
    };
    for (check_key, state) in [
        (key, ItemState::Enabled),
        (&format!("_disabled_{}", key), ItemState::Disabled),
    ] {
        if let Some(obj) = doc.get(check_key).and_then(|v| v.as_object()) {
            for (name, value) in obj {
                let mut item = ConfigItem::new(name.clone(), kind, path.to_owned(), provider);
                item.state = state;
                item.editable = false;
                item.detail = Some(json_detail(value));
                out.push(item);
            }
        }
    }
    out
}

fn json_detail(value: &serde_json::Value) -> String {
    serde_json::to_string(&canonical_json(value)).unwrap_or_else(|_| value.to_string())
}

fn toml_detail(value: &toml::Value) -> String {
    json_detail(&toml_to_json(value))
}

fn canonical_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(canonical_json).collect())
        }
        serde_json::Value::Object(obj) => {
            let mut sorted = serde_json::Map::new();
            let mut keys: Vec<_> = obj.keys().collect();
            keys.sort();
            for key in keys {
                sorted.insert(key.clone(), canonical_json(&obj[key]));
            }
            serde_json::Value::Object(sorted)
        }
        _ => value.clone(),
    }
}

fn toml_to_json(value: &toml::Value) -> serde_json::Value {
    match value {
        toml::Value::String(v) => serde_json::Value::String(v.clone()),
        toml::Value::Integer(v) => serde_json::json!(v),
        toml::Value::Float(v) => serde_json::json!(v),
        toml::Value::Boolean(v) => serde_json::json!(v),
        toml::Value::Datetime(v) => serde_json::Value::String(v.to_string()),
        toml::Value::Array(arr) => serde_json::Value::Array(arr.iter().map(toml_to_json).collect()),
        toml::Value::Table(table) => {
            let mut obj = serde_json::Map::new();
            let mut keys: Vec<_> = table.keys().collect();
            keys.sort();
            for key in keys {
                obj.insert(key.clone(), toml_to_json(&table[key]));
            }
            serde_json::Value::Object(obj)
        }
    }
}

fn scan_toml_mcp(path: &Path, provider: ProviderId) -> Vec<ConfigItem> {
    let mut out = vec![];
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        _ => return out,
    };
    let doc: toml::Value = match toml::from_str(&text) {
        Ok(d) => d,
        _ => return out,
    };
    let servers = match doc.get("mcp_servers").and_then(|v| v.as_table()) {
        Some(t) => t,
        _ => return out,
    };
    for (name, value) in servers {
        let mut item = ConfigItem::new(name.clone(), ItemKind::Mcp, path.to_owned(), provider);
        item.editable = false;
        item.detail = Some(toml_detail(value));
        if value.get("enabled").and_then(|v| v.as_bool()) == Some(false) {
            item.state = ItemState::Disabled;
        }
        out.push(item);
    }
    out
}

fn scan_toml_hooks(path: &Path, provider: ProviderId) -> Vec<ConfigItem> {
    let mut out = vec![];
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        _ => return out,
    };
    let doc: toml::Value = match toml::from_str(&text) {
        Ok(d) => d,
        _ => return out,
    };
    let hooks = match doc.get("hooks").and_then(|v| v.as_table()) {
        Some(t) => t,
        _ => return out,
    };
    for (event, entries) in hooks {
        if event.ends_with("managed_dir") {
            continue;
        }
        let arr = match entries.as_array() {
            Some(a) => a,
            _ => continue,
        };
        for (i, entry) in arr.iter().enumerate() {
            let matcher = entry.get("matcher").and_then(|v| v.as_str()).unwrap_or("*");
            let hook_name = entry
                .get("hooks")
                .and_then(|h| h.as_array())
                .and_then(|a| a.first())
                .and_then(|h| h.get("name").or_else(|| h.get("command")))
                .and_then(|n| n.as_str())
                .map(String::from);
            let display = hook_name
                .clone()
                .unwrap_or_else(|| format!("{}: {}", event, matcher));
            let loc = HookLoc {
                event: event.clone(),
                index: i,
                hook_name: hook_name.unwrap_or_else(|| matcher.to_string()),
            };
            let mut item = ConfigItem::new(display, ItemKind::Hook, path.to_owned(), provider);
            item.hook_loc = Some(loc);
            item.editable = false;
            item.detail = Some(toml_detail(entry));
            out.push(item);
        }
    }
    out
}

fn scan_hook_entries(
    path: &Path,
    provider: ProviderId,
    disabled_names: &[String],
) -> Vec<ConfigItem> {
    let mut out = vec![];
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        _ => return out,
    };
    let doc: serde_json::Value = match serde_json::from_str(&text) {
        Ok(d) => d,
        _ => return out,
    };
    let hooks_obj = match doc.get("hooks").and_then(|v| v.as_object()) {
        Some(o) => o,
        _ => return out,
    };

    for (event, entries) in hooks_obj {
        if event == "disabled" || event.starts_with("_agentswitch") {
            continue;
        }
        let arr = match entries.as_array() {
            Some(a) => a,
            _ => continue,
        };
        for (i, entry) in arr.iter().enumerate() {
            let matcher = entry.get("matcher").and_then(|v| v.as_str()).unwrap_or("*");
            let hook_name = entry
                .get("hooks")
                .and_then(|h| h.as_array())
                .and_then(|a| a.first())
                .and_then(|h| h.get("name").and_then(|n| n.as_str()))
                .map(String::from);
            let display = hook_name
                .clone()
                .unwrap_or_else(|| format!("{}: {}", event, matcher));
            let is_disabled = hook_name
                .as_ref()
                .is_some_and(|n| disabled_names.contains(n));
            let loc = HookLoc {
                event: event.clone(),
                index: i,
                hook_name: hook_name.unwrap_or_else(|| matcher.to_string()),
            };
            let mut item = ConfigItem::new(display, ItemKind::Hook, path.to_owned(), provider);
            item.hook_loc = Some(loc);
            item.editable = false;
            item.detail = Some(json_detail(entry));
            if is_disabled {
                item.state = ItemState::Disabled;
            }
            out.push(item);
        }
    }
    if let Some(stashed) = doc.get("_agentswitch_disabled").and_then(|v| v.as_object()) {
        for (event, entries) in stashed {
            let arr = match entries.as_array() {
                Some(a) => a,
                _ => continue,
            };
            for (i, entry) in arr.iter().enumerate() {
                let matcher = entry.get("matcher").and_then(|v| v.as_str()).unwrap_or("*");
                let hook_name = entry
                    .get("hooks")
                    .and_then(|h| h.as_array())
                    .and_then(|a| a.first())
                    .and_then(|h| h.get("name").and_then(|n| n.as_str()))
                    .map(String::from);
                let display = hook_name
                    .clone()
                    .unwrap_or_else(|| format!("{}: {}", event, matcher));
                let loc = HookLoc {
                    event: format!("_stashed_{}", event),
                    index: i,
                    hook_name: hook_name.unwrap_or_else(|| matcher.to_string()),
                };
                let mut item = ConfigItem::new(display, ItemKind::Hook, path.to_owned(), provider);
                item.hook_loc = Some(loc);
                item.state = ItemState::Disabled;
                item.editable = false;
                item.detail = Some(json_detail(entry));
                out.push(item);
            }
        }
    }
    out
}

fn gemini_disabled_names(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|d| d.get("hooks")?.get("disabled")?.as_array().cloned())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn scan_claude(root: &Path, scope: Scope) -> Vec<ConfigItem> {
    let d = provider_dir(ProviderId::Claude, root, scope);
    let mut items = vec![];
    match scope {
        Scope::Project => items.extend(check_file(
            root.join("CLAUDE.md"),
            ItemKind::InstructionFile,
            ProviderId::Claude,
        )),
        Scope::Global => items.extend(check_file(
            d.join("CLAUDE.md"),
            ItemKind::InstructionFile,
            ProviderId::Claude,
        )),
    }
    items.extend(collect_subdirs(
        &d.join("skills"),
        ItemKind::Skill,
        ProviderId::Claude,
    ));
    items.extend(collect_md(
        &d.join("rules"),
        ItemKind::Rule,
        ProviderId::Claude,
    ));
    let settings = d.join("settings.json");
    items.extend(scan_hook_entries(&settings, ProviderId::Claude, &[]));
    items.extend(scan_json_keys(
        &settings,
        "mcpServers",
        ItemKind::Mcp,
        ProviderId::Claude,
    ));
    items
}

fn scan_codex(root: &Path, scope: Scope) -> Vec<ConfigItem> {
    let d = provider_dir(ProviderId::Codex, root, scope);
    let mut items = vec![];
    if scope == Scope::Project {
        items.extend(check_file(
            root.join("AGENTS.md"),
            ItemKind::InstructionFile,
            ProviderId::Codex,
        ));
        items.extend(collect_subdirs(
            &root.join(".agents").join("skills"),
            ItemKind::Skill,
            ProviderId::Codex,
        ));
    }
    items.extend(collect_subdirs(
        &d.join("skills"),
        ItemKind::Skill,
        ProviderId::Codex,
    ));
    let config = d.join("config.toml");
    items.extend(scan_toml_mcp(&config, ProviderId::Codex));
    items.extend(scan_toml_hooks(&config, ProviderId::Codex));
    if scope == Scope::Project {
        items.extend(scan_json_keys(
            &root.join(".mcp.json"),
            "mcpServers",
            ItemKind::Mcp,
            ProviderId::Codex,
        ));
    }
    let hooks = d.join("hooks.json");
    if hooks.exists() {
        items.extend(scan_hook_entries(&hooks, ProviderId::Codex, &[]));
    }
    let hooks_dis = PathBuf::from(format!("{}.disabled", hooks.display()));
    if hooks_dis.exists() {
        items.push(ConfigItem::new(
            "hooks.json (disabled)",
            ItemKind::Hook,
            hooks_dis,
            ProviderId::Codex,
        ));
    }
    items
}

fn scan_gemini(root: &Path, scope: Scope) -> Vec<ConfigItem> {
    let d = provider_dir(ProviderId::Gemini, root, scope);
    let mut items = vec![];
    if scope == Scope::Project {
        items.extend(check_file(
            root.join("GEMINI.md"),
            ItemKind::InstructionFile,
            ProviderId::Gemini,
        ));
        items.extend(check_file(
            root.join("AGENTS.md"),
            ItemKind::InstructionFile,
            ProviderId::Gemini,
        ));
    }
    items.extend(collect_subdirs(
        &d.join("skills"),
        ItemKind::Skill,
        ProviderId::Gemini,
    ));
    items.extend(collect_md(
        &d.join("rules"),
        ItemKind::Rule,
        ProviderId::Gemini,
    ));
    let settings = d.join("settings.json");
    let disabled = gemini_disabled_names(&settings);
    items.extend(scan_hook_entries(&settings, ProviderId::Gemini, &disabled));
    items.extend(scan_json_keys(
        &settings,
        "mcpServers",
        ItemKind::Mcp,
        ProviderId::Gemini,
    ));
    items
}

fn scan_kiro(root: &Path, scope: Scope) -> Vec<ConfigItem> {
    let d = provider_dir(ProviderId::Kiro, root, scope);
    let mut items = vec![];
    items.extend(collect_md(
        &d.join("steering"),
        ItemKind::SteeringRule,
        ProviderId::Kiro,
    ));
    items.extend(collect_subdirs(
        &d.join("specs"),
        ItemKind::Spec,
        ProviderId::Kiro,
    ));
    items.extend(collect_subdirs(
        &d.join("agents"),
        ItemKind::Agent,
        ProviderId::Kiro,
    ));
    let agents_dir = d.join("agents");
    if agents_dir.is_dir() {
        if let Ok(rd) = std::fs::read_dir(&agents_dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().and_then(|e| e.to_str()) == Some("json") {
                    items.extend(scan_hook_entries(&p, ProviderId::Kiro, &[]));
                }
            }
        }
    }
    items.extend(scan_json_keys(
        &d.join("settings").join("mcp.json"),
        "mcpServers",
        ItemKind::Mcp,
        ProviderId::Kiro,
    ));
    items
}

fn scan_opencode(root: &Path, scope: Scope) -> Vec<ConfigItem> {
    let d = provider_dir(ProviderId::OpenCode, root, scope);
    let mut items = vec![];
    if scope == Scope::Project {
        items.extend(check_file(
            root.join("AGENTS.md"),
            ItemKind::InstructionFile,
            ProviderId::OpenCode,
        ));
    }
    items.extend(collect_subdirs(
        &d.join("skills"),
        ItemKind::Skill,
        ProviderId::OpenCode,
    ));
    let cfg = if scope == Scope::Global {
        d.join("opencode.json")
    } else {
        root.join("opencode.json")
    };
    let cfg_jsonc = cfg.with_extension("jsonc");
    let actual_cfg = if cfg_jsonc.exists() { cfg_jsonc } else { cfg };
    items.extend(scan_json_keys(
        &actual_cfg,
        "agent",
        ItemKind::Agent,
        ProviderId::OpenCode,
    ));
    items.extend(scan_json_keys(
        &actual_cfg,
        "mcp",
        ItemKind::Mcp,
        ProviderId::OpenCode,
    ));
    if let Ok(text) = std::fs::read_to_string(&actual_cfg) {
        if let Ok(doc) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(plugins) = doc.get("plugin").and_then(|v| v.as_array()) {
                for (i, p) in plugins.iter().enumerate() {
                    let name = match p {
                        serde_json::Value::String(s) => s.clone(),
                        serde_json::Value::Array(a) => a
                            .first()
                            .and_then(|v| v.as_str())
                            .unwrap_or("plugin")
                            .to_string(),
                        _ => continue,
                    };
                    let mut item = ConfigItem::new(
                        name,
                        ItemKind::Hook,
                        actual_cfg.clone(),
                        ProviderId::OpenCode,
                    );
                    item.hook_loc = Some(HookLoc {
                        event: "plugin".into(),
                        index: i,
                        hook_name: String::new(),
                    });
                    item.editable = false;
                    item.detail = Some(json_detail(p));
                    items.push(item);
                }
            }
        }
    }
    items
}
