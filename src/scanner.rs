use crate::process::{self, CliProbe};
use crate::provider;
use crate::types::*;
use std::path::{Path, PathBuf};

pub fn scan_provider(id: ProviderId, root: &Path, scope: Scope) -> Vec<ConfigItem> {
    deduplicate_items(match id {
        ProviderId::Claude => scan_claude(root, scope),
        ProviderId::Codex => scan_codex(root, scope),
        ProviderId::Antigravity => scan_antigravity(root, scope),
        ProviderId::Kiro => scan_kiro(root, scope),
        ProviderId::OpenCode => scan_opencode(root, scope),
        ProviderId::Zcode => scan_zcode(root, scope),
    })
}

pub fn provider_exists(id: ProviderId, root: &Path, scope: Scope) -> bool {
    let configured = provider::provider_dir(id, root, scope).is_ok_and(|path| path.is_dir());
    let shared_project_path = scope == Scope::Project
        && matches!(
            id,
            ProviderId::Codex | ProviderId::Antigravity | ProviderId::Zcode
        )
        && root.join(".agents").is_dir();
    configured
        || shared_project_path
        || provider::cli_names(id)
            .iter()
            .any(|name| process::shared().probe(name).installed)
}

pub fn provider_dir(id: ProviderId, root: &Path, scope: Scope) -> anyhow::Result<PathBuf> {
    provider::provider_dir(id, root, scope)
}

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

fn collect_md_both(dir: &Path, kind: ItemKind, provider: ProviderId) -> Vec<ConfigItem> {
    let mut out = collect_md(dir, kind, provider);
    let mut dis_dir = dir.to_path_buf();
    if let Some(name) = dir.file_name() {
        dis_dir.set_file_name(format!("{}.disabled", name.to_string_lossy()));
        out.extend(collect_md(&dis_dir, kind, provider));
    }
    out
}

fn collect_subdirs_both(dir: &Path, kind: ItemKind, provider: ProviderId) -> Vec<ConfigItem> {
    let mut out = collect_subdirs(dir, kind, provider);
    let mut dis_dir = dir.to_path_buf();
    if let Some(name) = dir.file_name() {
        let mut disabled_name = name.to_os_string();
        disabled_name.push(".disabled");
        dis_dir.set_file_name(disabled_name);
        out.extend(collect_subdirs(&dis_dir, kind, provider));
    }
    out
}

fn deduplicate_items(items: Vec<ConfigItem>) -> Vec<ConfigItem> {
    let mut seen = std::collections::HashSet::new();
    items
        .into_iter()
        .filter(|item| {
            seen.insert((
                item.kind,
                item.state,
                item.path.clone(),
                item.name.clone(),
                item.hook_loc.as_ref().map(|l| {
                    (
                        l.section.clone(),
                        l.event.clone(),
                        l.order,
                        l.fingerprint.clone(),
                    )
                }),
            ))
        })
        .collect()
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
        out.push(ConfigItem::new(name, kind, dis, provider));
    }
    out
}

fn read_string_lists(
    path: &Path,
    enabled_key: &str,
    disabled_key: &str,
) -> (Vec<String>, Vec<String>) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return (vec![], vec![]);
    };
    let Ok(doc) = serde_json::from_str::<serde_json::Value>(&text) else {
        return (vec![], vec![]);
    };
    let read = |key: &str| {
        doc.get(key)
            .and_then(|value| value.as_array())
            .into_iter()
            .flatten()
            .filter_map(|value| value.as_str().map(String::from))
            .collect()
    };
    (read(enabled_key), read(disabled_key))
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn read_toml(path: &Path) -> Option<toml::Value> {
    toml::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn scan_json_keys(path: &Path, key: &str, kind: ItemKind, provider: ProviderId) -> Vec<ConfigItem> {
    let mut out = vec![];
    let Some(doc) = read_json(path) else {
        return out;
    };
    for (check_key, base_state) in [
        (key, ItemState::Enabled),
        (&format!("_disabled_{}", key), ItemState::Disabled),
    ] {
        if let Some(obj) = doc.get(check_key).and_then(|v| v.as_object()) {
            for (name, value) in obj {
                let mut item = ConfigItem::new(name.clone(), kind, path.to_owned(), provider);
                item.state = if base_state == ItemState::Disabled
                    || value.get("disabled").and_then(|v| v.as_bool()) == Some(true)
                    || value.get("enabled").and_then(|v| v.as_bool()) == Some(false)
                {
                    ItemState::Disabled
                } else {
                    ItemState::Enabled
                };
                item.editable = false;
                item.toggle_spec = Some(match (provider, key) {
                    (ProviderId::OpenCode, "mcp" | "agent") => ToggleSpec::JsonFlag {
                        section: key.to_string(),
                        name: name.clone(),
                        flag: "enabled".into(),
                        enabled_value: true,
                        disabled_value: false,
                    },
                    (ProviderId::Antigravity | ProviderId::Kiro, "mcpServers") => {
                        ToggleSpec::JsonFlag {
                            section: key.to_string(),
                            name: name.clone(),
                            flag: "disabled".into(),
                            enabled_value: false,
                            disabled_value: true,
                        }
                    }
                    (ProviderId::Claude, "mcpServers") => ToggleSpec::JsonStash {
                        section: key.to_string(),
                        name: name.clone(),
                    },
                    _ => ToggleSpec::JsonStash {
                        section: key.to_string(),
                        name: name.clone(),
                    },
                });
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

fn json_fingerprint(value: &serde_json::Value) -> String {
    serde_json::to_string(&canonical_json(value)).unwrap_or_else(|_| value.to_string())
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
    let Some(doc) = read_toml(path) else {
        return out;
    };
    let servers = match doc.get("mcp_servers").and_then(|v| v.as_table()) {
        Some(t) => t,
        _ => return out,
    };
    for (name, value) in servers {
        let mut item = ConfigItem::new(name.clone(), ItemKind::Mcp, path.to_owned(), provider);
        item.editable = false;
        item.toggle_spec = Some(ToggleSpec::TomlFlag {
            section: "mcp_servers".into(),
            name: name.clone(),
            flag: "enabled".into(),
            enabled_value: true,
            disabled_value: false,
        });
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
    let Some(doc) = read_toml(path) else {
        return out;
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
        for (order, entry) in arr.iter().enumerate() {
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
                section: "hooks".into(),
                event: event.clone(),
                order,
                hook_name: hook_name.unwrap_or_else(|| matcher.to_string()),
                fingerprint: json_fingerprint(&toml_to_json(entry)),
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

fn collect_hook_items(
    entries_iter: impl Iterator<Item = (String, serde_json::Value)>,
    path: &Path,
    provider: ProviderId,
    section_path: &str,
    disabled_names: &[String],
    event_prefix: &str,
    force_disabled: bool,
) -> Vec<ConfigItem> {
    let mut out = vec![];
    for (event, entries) in entries_iter {
        let arr = match entries.as_array() {
            Some(a) => a,
            _ => continue,
        };
        for (order, entry) in arr.iter().enumerate() {
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
            let entry_flag_disabled = provider == ProviderId::Zcode
                && entry.get("enabled").and_then(|v| v.as_bool()) == Some(false);
            let is_disabled = force_disabled
                || entry_flag_disabled
                || hook_name.as_ref().is_some_and(|n| {
                    disabled_names.contains(n)
                        || disabled_names.contains(&format!("{}:{}", event, n))
                });
            let loc = HookLoc {
                section: section_path.to_string(),
                event: format!("{}{}", event_prefix, event),
                order,
                hook_name: hook_name.unwrap_or_else(|| matcher.to_string()),
                fingerprint: match provider {
                    ProviderId::Zcode => crate::toggler::zcode_entry_fingerprint(entry),
                    _ if event_prefix == "_stashed_" => {
                        crate::toggler::stash_entry_fingerprint(entry)
                    }
                    _ => json_fingerprint(entry),
                },
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
    out
}

fn scan_stash_doc(path: &Path, provider: ProviderId, section_path: &str) -> Vec<ConfigItem> {
    let mut out = vec![];
    let stash_path = crate::toggler::sidecar_path(path);
    let Some(doc) = read_json(&stash_path) else {
        return out;
    };
    if let Some(stashed) = doc.as_object() {
        let mapped = stashed.iter().map(|(e, v)| (e.clone(), v.clone()));
        out.extend(collect_hook_items(
            mapped,
            path,
            provider,
            section_path,
            &[],
            "_stashed_",
            true,
        ));
    }
    out
}

fn scan_hook_entries(
    path: &Path,
    provider: ProviderId,
    section_path: &str,
    disabled_names: &[String],
    force_disabled: bool,
) -> Vec<ConfigItem> {
    let mut out = vec![];
    let Some(doc) = read_json(path) else {
        return out;
    };
    if let Some(hooks_obj) = json_at(&doc, section_path).and_then(|v| v.as_object()) {
        let filtered = hooks_obj
            .iter()
            .filter(|(event, _)| event.as_str() != "disabled" && !event.starts_with("_agentswitch"))
            .map(|(e, v)| (e.clone(), v.clone()));
        out.extend(collect_hook_items(
            filtered,
            path,
            provider,
            section_path,
            disabled_names,
            "",
            force_disabled,
        ));
    }
    if let Some(stashed) = doc.get("_agentswitch_disabled").and_then(|v| v.as_object()) {
        let mapped = stashed.iter().map(|(e, v)| (e.clone(), v.clone()));
        out.extend(collect_hook_items(
            mapped,
            path,
            provider,
            section_path,
            &[],
            "_stashed_",
            true,
        ));
    }
    out.extend(scan_stash_doc(path, provider, section_path));
    out
}

fn scan_antigravity_hooks(path: &Path) -> Vec<ConfigItem> {
    let mut out = vec![];
    let Some(doc) = read_json(path) else {
        return out;
    };
    let Some(root) = doc.as_object() else {
        return out;
    };
    for (name, def) in root {
        if name == "disabled" || name == "hooks" {
            continue;
        }
        let Some(def) = def.as_object() else {
            continue;
        };
        let Some(event) = def
            .keys()
            .find(|k| def.get(*k).is_some_and(|v| v.is_array()))
        else {
            continue;
        };
        let mut item = ConfigItem::new(
            name.clone(),
            ItemKind::Hook,
            path.to_owned(),
            ProviderId::Antigravity,
        );
        item.editable = false;
        item.detail = Some(json_detail(&serde_json::Value::Object(def.clone())));
        if def.get("enabled").and_then(|v| v.as_bool()) == Some(false) {
            item.state = ItemState::Disabled;
        }
        item.hook_loc = Some(HookLoc {
            section: String::new(),
            event: event.clone(),
            order: 0,
            hook_name: name.clone(),
            fingerprint: hook_def_fingerprint(&serde_json::Value::Object(def.clone())),
        });
        out.push(item);
    }
    out
}

fn hook_def_fingerprint(def: &serde_json::Value) -> String {
    let mut stripped = def.clone();
    if let Some(obj) = stripped.as_object_mut() {
        obj.remove("enabled");
    }
    json_fingerprint(&stripped)
}

fn scan_kiro_hook_file(path: &Path) -> Vec<ConfigItem> {
    let mut out = vec![];
    let Some(doc) = read_json(path) else {
        return out;
    };
    let Some(entries) = doc.get("hooks").and_then(|v| v.as_array()) else {
        return out;
    };
    for (order, entry) in entries.iter().enumerate() {
        let Some(obj) = entry.as_object() else {
            continue;
        };
        let trigger = obj.get("trigger").and_then(|v| v.as_str()).unwrap_or("");
        let name = obj
            .get("name")
            .and_then(|v| v.as_str())
            .filter(|n| !n.is_empty())
            .map(String::from)
            .unwrap_or_else(|| format!("hook {}", order + 1));
        let mut item = ConfigItem::new(name, ItemKind::Hook, path.to_owned(), ProviderId::Kiro);
        item.editable = false;
        item.detail = Some(json_detail(entry));
        if obj.get("enabled").and_then(|v| v.as_bool()) == Some(false) {
            item.state = ItemState::Disabled;
        }
        item.hook_loc = Some(HookLoc {
            section: "hooks".into(),
            event: trigger.into(),
            order,
            hook_name: obj
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .into(),
            fingerprint: crate::toggler::kiro_entry_fingerprint(entry),
        });
        out.push(item);
    }
    out
}

fn scan_kiro_hook_files(d: &Path) -> Vec<ConfigItem> {
    let mut out = vec![];
    let Ok(rd) = std::fs::read_dir(d.join("hooks")) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        if !p.is_file() {
            continue;
        }
        let name = p.file_name().unwrap_or_default().to_string_lossy();
        if name.ends_with(".json") || name.ends_with(".kiro.hook") {
            out.extend(scan_kiro_hook_file(&p));
        }
    }
    out
}

fn json_at<'a>(doc: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let mut current = doc;
    for segment in path.split('/').filter(|s| !s.is_empty()) {
        current = current.get(segment)?;
    }
    Some(current)
}

fn scan_claude(root: &Path, scope: Scope) -> Vec<ConfigItem> {
    let Ok(d) = provider_dir(ProviderId::Claude, root, scope) else {
        return vec![];
    };
    let mut items = vec![];
    let instructions: &Path = if scope == Scope::Project { root } else { &d };
    items.extend(check_file(
        instructions.join("CLAUDE.md"),
        ItemKind::InstructionFile,
        ProviderId::Claude,
    ));
    items.extend(collect_subdirs_both(
        &d.join("skills"),
        ItemKind::Skill,
        ProviderId::Claude,
    ));
    items.extend(collect_md_both(
        &d.join("rules"),
        ItemKind::Rule,
        ProviderId::Claude,
    ));
    let settings = d.join("settings.json");
    items.extend(scan_hook_entries(
        &settings,
        ProviderId::Claude,
        "hooks",
        &[],
        false,
    ));
    let mcp_path = match scope {
        Scope::Project => root.join(".mcp.json"),
        Scope::Global => provider::home_dir()
            .map(|home| home.join(".claude.json"))
            .unwrap_or_default(),
    };
    let mut mcp_items = scan_json_keys(&mcp_path, "mcpServers", ItemKind::Mcp, ProviderId::Claude);
    if scope == Scope::Project {
        let approval_path = d.join("settings.local.json");
        let approval_path = if approval_path.exists() {
            approval_path
        } else {
            d.join("settings.json")
        };
        let approval = read_string_lists(
            &approval_path,
            "enabledMcpjsonServers",
            "disabledMcpjsonServers",
        );
        for item in &mut mcp_items {
            if approval.1.contains(&item.name) {
                item.state = ItemState::Disabled;
            }
            item.toggle_spec = Some(ToggleSpec::StringLists {
                path: approval_path.clone(),
                enabled_key: "enabledMcpjsonServers".into(),
                disabled_key: "disabledMcpjsonServers".into(),
                name: item.name.clone(),
            });
        }
    }
    items.extend(mcp_items);
    items
}

fn scan_codex(root: &Path, scope: Scope) -> Vec<ConfigItem> {
    let Ok(d) = provider_dir(ProviderId::Codex, root, scope) else {
        return vec![];
    };
    let mut items = vec![];
    if scope == Scope::Project {
        items.extend(check_file(
            root.join("AGENTS.md"),
            ItemKind::InstructionFile,
            ProviderId::Codex,
        ));
        items.extend(collect_subdirs_both(
            &root.join(".agents").join("skills"),
            ItemKind::Skill,
            ProviderId::Codex,
        ));
    } else {
        items.extend(check_file(
            d.join("AGENTS.md"),
            ItemKind::InstructionFile,
            ProviderId::Codex,
        ));
    }
    items.extend(collect_subdirs_both(
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
        items.extend(scan_hook_entries(
            &hooks,
            ProviderId::Codex,
            "hooks",
            &[],
            false,
        ));
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

fn scan_antigravity(root: &Path, scope: Scope) -> Vec<ConfigItem> {
    let mut items = vec![];
    let Ok(d) = provider_dir(ProviderId::Antigravity, root, scope) else {
        return vec![];
    };
    if scope == Scope::Project {
        items.extend(check_file(
            root.join("GEMINI.md"),
            ItemKind::InstructionFile,
            ProviderId::Antigravity,
        ));
        items.extend(check_file(
            root.join("AGENTS.md"),
            ItemKind::InstructionFile,
            ProviderId::Antigravity,
        ));
        items.extend(collect_subdirs_both(
            &root.join(".agents").join("skills"),
            ItemKind::Skill,
            ProviderId::Antigravity,
        ));
    } else {
        items.extend(check_file(
            d.join("GEMINI.md"),
            ItemKind::InstructionFile,
            ProviderId::Antigravity,
        ));
        items.extend(check_file(
            d.join("AGENTS.md"),
            ItemKind::InstructionFile,
            ProviderId::Antigravity,
        ));
        items.extend(collect_subdirs_both(
            &d.join("skills"),
            ItemKind::Skill,
            ProviderId::Antigravity,
        ));
    }
    items.extend(scan_json_keys(
        &d.join("mcp_config.json"),
        "mcpServers",
        ItemKind::Mcp,
        ProviderId::Antigravity,
    ));
    let hooks_path = d.join("hooks.json");
    let legacy_wrapper = read_json(&hooks_path)
        .is_some_and(|doc| doc.get("hooks").and_then(|v| v.as_object()).is_some());
    if legacy_wrapper {
        items.extend(scan_hook_entries(
            &hooks_path,
            ProviderId::Antigravity,
            "hooks",
            &[],
            false,
        ));
    } else {
        items.extend(scan_antigravity_hooks(&hooks_path));
    }
    items
}

fn scan_kiro(root: &Path, scope: Scope) -> Vec<ConfigItem> {
    let Ok(d) = provider_dir(ProviderId::Kiro, root, scope) else {
        return vec![];
    };
    let mut items = vec![];
    items.extend(collect_md_both(
        &d.join("steering"),
        ItemKind::SteeringRule,
        ProviderId::Kiro,
    ));
    items.extend(collect_subdirs_both(
        &d.join("specs"),
        ItemKind::Spec,
        ProviderId::Kiro,
    ));
    items.extend(collect_subdirs_both(
        &d.join("agents"),
        ItemKind::Agent,
        ProviderId::Kiro,
    ));

    for (agents_dir, force_disabled) in
        [(d.join("agents"), false), (d.join("agents.disabled"), true)]
    {
        if agents_dir.is_dir() {
            if let Ok(rd) = std::fs::read_dir(&agents_dir) {
                for e in rd.flatten() {
                    let p = e.path();
                    if p.extension().and_then(|e| e.to_str()) == Some("json") {
                        items.extend(scan_hook_entries(
                            &p,
                            ProviderId::Kiro,
                            "hooks",
                            &[],
                            force_disabled,
                        ));
                    }
                }
            }
        }
    }
    items.extend(scan_kiro_hook_files(&d));
    items.extend(scan_json_keys(
        &d.join("settings").join("mcp.json"),
        "mcpServers",
        ItemKind::Mcp,
        ProviderId::Kiro,
    ));
    items
}

fn scan_opencode(root: &Path, scope: Scope) -> Vec<ConfigItem> {
    let Ok(d) = provider_dir(ProviderId::OpenCode, root, scope) else {
        return vec![];
    };
    let mut items = vec![];
    if scope == Scope::Project {
        items.extend(check_file(
            root.join("AGENTS.md"),
            ItemKind::InstructionFile,
            ProviderId::OpenCode,
        ));
    } else {
        items.extend(check_file(
            d.join("AGENTS.md"),
            ItemKind::InstructionFile,
            ProviderId::OpenCode,
        ));
    }
    items.extend(collect_subdirs_both(
        &d.join("skills"),
        ItemKind::Skill,
        ProviderId::OpenCode,
    ));
    if scope == Scope::Project {
        items.extend(collect_subdirs_both(
            &root.join(".agents").join("skills"),
            ItemKind::Skill,
            ProviderId::OpenCode,
        ));
        items.extend(collect_subdirs_both(
            &root.join(".claude").join("skills"),
            ItemKind::Skill,
            ProviderId::OpenCode,
        ));
    }
    if scope == Scope::Project {
        items.extend(collect_md_both(
            &d.join("agent"),
            ItemKind::Agent,
            ProviderId::OpenCode,
        ));
        items.extend(collect_md_both(
            &d.join("agents"),
            ItemKind::Agent,
            ProviderId::OpenCode,
        ));
    }
    let cfg = if scope == Scope::Global {
        d.join("opencode.json")
    } else {
        let flat = root.join("opencode.json");
        let nested = d.join("opencode.json");
        if !flat.exists() && !flat.with_extension("jsonc").exists() && nested.exists() {
            nested
        } else {
            flat
        }
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
                        ItemKind::Plugin,
                        actual_cfg.clone(),
                        ProviderId::OpenCode,
                    );
                    item.hook_loc = Some(HookLoc {
                        section: String::new(),
                        event: "plugin".into(),
                        order: i,
                        hook_name: String::new(),
                        fingerprint: format!("plugin:{i}"),
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

fn scan_zcode(root: &Path, scope: Scope) -> Vec<ConfigItem> {
    let Ok(d) = provider_dir(ProviderId::Zcode, root, scope) else {
        return vec![];
    };
    let mut items = vec![];
    items.extend(check_file(
        if scope == Scope::Project {
            root.join("AGENTS.md")
        } else {
            d.join("AGENTS.md")
        },
        ItemKind::InstructionFile,
        ProviderId::Zcode,
    ));
    items.extend(collect_subdirs_both(
        &d.join("skills"),
        ItemKind::Skill,
        ProviderId::Zcode,
    ));
    let shared_skills = match scope {
        Scope::Project => root.join(".agents").join("skills"),
        Scope::Global => provider::home_dir()
            .map(|home| home.join(".agents").join("skills"))
            .unwrap_or_default(),
    };
    if !shared_skills.as_os_str().is_empty() {
        items.extend(collect_subdirs_both(
            &shared_skills,
            ItemKind::Skill,
            ProviderId::Zcode,
        ));
    }

    let config_path = match scope {
        Scope::Project => {
            let nested = d.join("config.json");
            let flat = root.join("zcode.json");
            if !nested.exists() && flat.exists() {
                flat
            } else {
                nested
            }
        }
        Scope::Global => d.join("cli").join("config.json"),
    };
    let primary_servers = scan_json_keys_at(&config_path, "mcp/servers", ProviderId::Zcode);
    let has_primary_servers = !primary_servers.is_empty();
    items.extend(primary_servers);
    if !has_primary_servers {
        let fallback_mcp = match scope {
            Scope::Project => root.join(".agents").join("mcp.json"),
            Scope::Global => provider::home_dir()
                .map(|home| home.join(".agents").join("mcp.json"))
                .unwrap_or_default(),
        };
        if !fallback_mcp.as_os_str().is_empty() {
            items.extend(scan_json_keys(
                &fallback_mcp,
                "mcpServers",
                ItemKind::Mcp,
                ProviderId::Zcode,
            ));
        }
    }
    if config_path.is_file() {
        items.extend(scan_hook_entries(
            &config_path,
            ProviderId::Zcode,
            "hooks/events",
            &[],
            false,
        ));
    }
    items
}

fn scan_json_keys_at(path: &Path, section_path: &str, provider: ProviderId) -> Vec<ConfigItem> {
    let mut out = vec![];
    let Some(doc) = read_json(path) else {
        return out;
    };
    let stash_key = format!("_disabled_{}", section_path.replace('/', "_"));
    for (section, base_state) in [
        (section_path, ItemState::Enabled),
        (&stash_key, ItemState::Disabled),
    ] {
        let Some(servers) = json_at(&doc, section).and_then(|v| v.as_object()) else {
            continue;
        };
        for (name, value) in servers {
            let mut item = ConfigItem::new(name.clone(), ItemKind::Mcp, path.to_owned(), provider);
            item.state = base_state;
            item.editable = false;
            item.toggle_spec = Some(ToggleSpec::JsonStash {
                section: section_path.to_string(),
                name: name.clone(),
            });
            item.detail = Some(json_detail(value));
            out.push(item);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("agentswitch-scanner-{name}-{nonce}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn claude_project_mcp_comes_from_dot_mcp_json() {
        let root = temp_dir("claude-mcp");
        std::fs::write(
            root.join(".mcp.json"),
            r#"{"mcpServers":{"docs":{"type":"http","url":"https://example.test"}}}"#,
        )
        .unwrap();
        std::fs::create_dir_all(root.join(".claude")).unwrap();
        std::fs::write(
            root.join(".claude").join("settings.json"),
            r#"{"mcpServers":{"stale":{"command":"old"}}}"#,
        )
        .unwrap();

        let items = scan_provider(ProviderId::Claude, &root, Scope::Project);
        let mcps: Vec<_> = items
            .iter()
            .filter(|item| item.kind == ItemKind::Mcp)
            .collect();
        assert_eq!(mcps.len(), 1);
        assert_eq!(mcps[0].name, "docs");
        assert_eq!(mcps[0].path, root.join(".mcp.json"));
    }

    #[test]
    fn antigravity_scans_project_hooks() {
        let root = temp_dir("antigravity-project");
        let agents = root.join(".agents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(
            agents.join("hooks.json"),
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"command":"check"}]}]}}"#,
        )
        .unwrap();

        let items = scan_provider(ProviderId::Antigravity, &root, Scope::Project);
        assert!(items.iter().any(|item| item.kind == ItemKind::Hook));
    }

    #[test]
    fn antigravity_disabled_hooks_survive_a_rescan() {
        let root = temp_dir("antigravity-state");
        let agents = root.join(".agents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(
            agents.join("hooks.json"),
            r#"{
                "safety-gate": {
                    "enabled": false,
                    "PreToolUse": [{"matcher": "Bash", "hooks": [{"command": "check"}]}]
                },
                "linter": {
                    "PostToolUse": [{"matcher": "Bash", "hooks": [{"command": "check"}]}]
                }
            }"#,
        )
        .unwrap();

        let items = scan_provider(ProviderId::Antigravity, &root, Scope::Project);
        let gate = items
            .iter()
            .find(|item| item.kind == ItemKind::Hook && item.name == "safety-gate")
            .expect("documented hook definition listed");
        assert_eq!(gate.state, ItemState::Disabled, "enabled:false honored");
        assert_eq!(gate.hook_loc.as_ref().unwrap().event, "PreToolUse");
        let linter = items
            .iter()
            .find(|item| item.kind == ItemKind::Hook && item.name == "linter")
            .expect("second definition listed");
        assert_eq!(linter.state, ItemState::Enabled);
    }

    #[test]
    fn antigravity_documented_hook_toggles_survive_a_rescan() {
        let root = temp_dir("antigravity-roundtrip");
        let agents = root.join(".agents");
        std::fs::create_dir_all(&agents).unwrap();
        let hooks_path = agents.join("hooks.json");
        std::fs::write(
            &hooks_path,
            r#"{"safety-gate":{"PreToolUse":[{"matcher":"Bash","hooks":[{"command":"check"}]}]}}"#,
        )
        .unwrap();

        let mut item = scan_provider(ProviderId::Antigravity, &root, Scope::Project)
            .into_iter()
            .find(|item| item.kind == ItemKind::Hook)
            .expect("hook discovered");
        assert_eq!(item.state, ItemState::Enabled);

        crate::toggler::toggle_item(&mut item).unwrap();
        let rescanned = scan_provider(ProviderId::Antigravity, &root, Scope::Project)
            .into_iter()
            .find(|item| item.kind == ItemKind::Hook)
            .expect("hook still listed");
        assert_eq!(rescanned.state, ItemState::Disabled);

        let mut item = rescanned;
        crate::toggler::toggle_item(&mut item).unwrap();
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&hooks_path).unwrap()).unwrap();
        assert!(
            doc["safety-gate"].get("enabled").is_none(),
            "re-enable must remove the flag instead of writing enabled:true"
        );
        assert_eq!(
            doc["safety-gate"]["PreToolUse"][0]["hooks"][0]["command"], "check",
            "definition content survives both directions"
        );
    }

    #[test]
    fn claude_hook_disable_rescan_enable_round_trip() {
        let root = temp_dir("claude-roundtrip");
        let claude = root.join(".claude");
        std::fs::create_dir_all(&claude).unwrap();
        let settings = claude.join("settings.json");
        let entry =
            serde_json::json!({"matcher":"Bash","hooks":[{"type":"command","command":"lint"}]});
        std::fs::write(
            &settings,
            serde_json::json!({"hooks":{"PreToolUse":[entry.clone()]}}).to_string(),
        )
        .unwrap();

        let mut item = scan_provider(ProviderId::Claude, &root, Scope::Project)
            .into_iter()
            .find(|i| i.kind == ItemKind::Hook)
            .expect("hook discovered");
        crate::toggler::toggle_item(&mut item).unwrap();

        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert!(
            doc.get("_agentswitch_disabled").is_none(),
            "settings.json must stay free of agentswitch keys for schema validation"
        );
        let mut rescanned = scan_provider(ProviderId::Claude, &root, Scope::Project)
            .into_iter()
            .find(|i| i.kind == ItemKind::Hook)
            .expect("stashed hook stays listed after rescan");
        assert_eq!(rescanned.state, ItemState::Disabled);

        crate::toggler::toggle_item(&mut rescanned).unwrap();
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(
            doc["hooks"]["PreToolUse"][0], entry,
            "entry restored verbatim"
        );
        assert!(
            !sidecar_path_exists(&settings),
            "sidecar removed once empty"
        );
    }

    #[test]
    fn codex_hooks_json_disable_rescan_enable_round_trip() {
        let root = temp_dir("codex-roundtrip");
        let codex = root.join(".codex");
        std::fs::create_dir_all(&codex).unwrap();
        let hooks_path = codex.join("hooks.json");
        let entry =
            serde_json::json!({"matcher":"Bash","hooks":[{"type":"command","command":"gate"}]});
        std::fs::write(
            &hooks_path,
            serde_json::json!({"description":"d","hooks":{"PreToolUse":[entry.clone()]}})
                .to_string(),
        )
        .unwrap();

        let mut item = scan_provider(ProviderId::Codex, &root, Scope::Project)
            .into_iter()
            .find(|i| i.kind == ItemKind::Hook)
            .expect("hook discovered");
        crate::toggler::toggle_item(&mut item).unwrap();
        let mut rescanned = scan_provider(ProviderId::Codex, &root, Scope::Project)
            .into_iter()
            .find(|i| i.kind == ItemKind::Hook)
            .expect("stashed hook stays listed after rescan");
        assert_eq!(rescanned.state, ItemState::Disabled);
        crate::toggler::toggle_item(&mut rescanned).unwrap();

        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&hooks_path).unwrap()).unwrap();
        assert_eq!(doc["hooks"]["PreToolUse"][0], entry);
        assert!(!sidecar_path_exists(&hooks_path));
    }

    #[test]
    fn kiro_native_hook_file_toggles_enabled_flag() {
        let root = temp_dir("kiro-roundtrip");
        let kiro = root.join(".kiro");
        std::fs::create_dir_all(kiro.join("hooks")).unwrap();
        let hook_file = kiro.join("hooks").join("lint-on-save.json");
        std::fs::write(
            &hook_file,
            r#"{"version":"v1","hooks":[
                {"name":"lint-on-save","trigger":"PostFileSave","matcher":"\\.ts$",
                 "action":{"type":"command","command":"npm run lint"},"enabled":true},
                {"name":"format-on-save","trigger":"PostFileSave",
                 "action":{"type":"command","command":"prettier --write {{filePath}}"}}
            ]}"#,
        )
        .unwrap();

        let mut items = scan_provider(ProviderId::Kiro, &root, Scope::Project)
            .into_iter()
            .filter(|i| i.kind == ItemKind::Hook)
            .collect::<Vec<_>>();
        assert_eq!(items.len(), 2, "both array entries listed");
        let lint = items
            .iter_mut()
            .find(|i| i.name == "lint-on-save")
            .expect("named hook listed");
        assert_eq!(lint.state, ItemState::Enabled);
        assert_eq!(lint.hook_loc.as_ref().unwrap().event, "PostFileSave");

        crate::toggler::toggle_item(lint).unwrap();
        let mut rescanned = scan_provider(ProviderId::Kiro, &root, Scope::Project)
            .into_iter()
            .filter(|i| i.kind == ItemKind::Hook)
            .collect::<Vec<_>>();
        let lint = rescanned
            .iter_mut()
            .find(|i| i.name == "lint-on-save")
            .expect("hook still listed");
        assert_eq!(
            lint.state,
            ItemState::Disabled,
            "rescan honors enabled:false"
        );
        crate::toggler::toggle_item(lint).unwrap();
        let fmt = rescanned
            .iter()
            .find(|i| i.name == "format-on-save")
            .expect("second hook listed");
        assert_eq!(fmt.state, ItemState::Enabled, "missing enabled defaults on");
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&hook_file).unwrap()).unwrap();
        assert!(doc["hooks"][0].get("enabled").is_none());
        assert!(doc["hooks"][1].get("enabled").is_none());
    }

    fn sidecar_path_exists(config: &Path) -> bool {
        crate::toggler::sidecar_path(config).exists()
    }

    #[test]
    fn legacy_in_file_stash_still_shows_and_reenables_after_rescan() {
        let root = temp_dir("legacy-roundtrip");
        let claude = root.join(".claude");
        std::fs::create_dir_all(&claude).unwrap();
        let settings = claude.join("settings.json");
        let entry =
            serde_json::json!({"matcher":"Bash","hooks":[{"type":"command","command":"old"}]});
        let stashed = serde_json::json!({
            "_agentswitch_order": 0,
            "matcher": "Bash",
            "hooks": [{"type": "command", "command": "old"}]
        });
        std::fs::write(
            &settings,
            serde_json::json!({
                "hooks":{"PreToolUse":[]},
                "_agentswitch_disabled":{"PreToolUse":[stashed]}
            })
            .to_string(),
        )
        .unwrap();

        let mut item = scan_provider(ProviderId::Claude, &root, Scope::Project)
            .into_iter()
            .find(|i| i.kind == ItemKind::Hook)
            .expect("legacy stashed hook listed");
        assert_eq!(item.state, ItemState::Disabled);
        crate::toggler::toggle_item(&mut item).unwrap();

        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(doc["hooks"]["PreToolUse"][0], entry);
        assert!(doc.get("_agentswitch_disabled").is_none());
    }

    #[test]
    fn hooks_on_different_events_with_the_same_name_stay_separate() {
        let root = temp_dir("hook-dedup");
        let claude = root.join(".claude");
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::write(
            claude.join("settings.json"),
            r#"{"hooks":{
                "PreToolUse":[{"matcher":"*","hooks":[{"command":"notify"}]}],
                "PostToolUse":[{"matcher":"*","hooks":[{"command":"notify"}]}]
            }}"#,
        )
        .unwrap();

        let items = scan_provider(ProviderId::Claude, &root, Scope::Project);
        let hooks: Vec<_> = items
            .iter()
            .filter(|item| item.kind == ItemKind::Hook)
            .collect();
        assert_eq!(hooks.len(), 2, "same command on two events is two hooks");
    }

    #[test]
    fn zcode_scans_config_skills_and_mcp() {
        let root = temp_dir("zcode-scan");
        let zc = root.join(".zcode");
        std::fs::create_dir_all(zc.join("skills").join("reviewer")).unwrap();
        std::fs::write(
            zc.join("config.json"),
            r#"{"hooks":{"enabled":true,"events":{"PostToolUse":[
                {"matcher":"Write","hooks":[{"type":"process","command":"lint"}]}
            ]}},"mcp":{"servers":{"docs":{"type":"stdio","command":"docs-mcp"}}}}"#,
        )
        .unwrap();
        std::fs::write(root.join("AGENTS.md"), "# project").unwrap();

        let items = scan_provider(ProviderId::Zcode, &root, Scope::Project);
        let skill = items
            .iter()
            .find(|i| i.kind == ItemKind::Skill && i.name == "reviewer")
            .expect("workspace skill discovered");
        assert_eq!(skill.state, ItemState::Enabled);
        let mcp = items
            .iter()
            .find(|i| i.kind == ItemKind::Mcp && i.name == "docs")
            .expect("mcp.servers discovered");
        assert_eq!(
            mcp.path,
            zc.join("config.json"),
            "server points at the workspace config"
        );
        let hook = items
            .iter()
            .find(|i| i.kind == ItemKind::Hook && i.name == "lint")
            .expect("hook under hooks/events discovered");
        let loc = hook.hook_loc.as_ref().unwrap();
        assert_eq!(loc.section, "hooks/events");
        assert_eq!(loc.event, "PostToolUse");
        let agents_md = items
            .iter()
            .find(|i| i.kind == ItemKind::InstructionFile)
            .expect("AGENTS.md discovered");
        assert!(agents_md.editable);
    }

    #[test]
    fn zcode_mcp_fallback_only_applies_without_primary_servers() {
        let root = temp_dir("zcode-fallback");
        let zc = root.join(".zcode");
        std::fs::create_dir_all(&zc).unwrap();
        std::fs::write(zc.join("config.json"), r#"{"hooks":{"enabled":false}}"#).unwrap();
        let agents = root.join(".agents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(
            agents.join("mcp.json"),
            r#"{"mcpServers":{"shared":{"command":"shared-mcp"}}}"#,
        )
        .unwrap();

        let items = scan_provider(ProviderId::Zcode, &root, Scope::Project);
        assert!(
            items
                .iter()
                .any(|i| i.kind == ItemKind::Mcp && i.name == "shared"),
            "compat fallback is honored when the primary config has no servers"
        );

        std::fs::write(
            zc.join("config.json"),
            r#"{"mcp":{"servers":{"native":{"command":"native-mcp"}}}}"#,
        )
        .unwrap();
        let items = scan_provider(ProviderId::Zcode, &root, Scope::Project);
        assert!(items
            .iter()
            .any(|i| i.kind == ItemKind::Mcp && i.name == "native"));
        assert!(
            !items
                .iter()
                .any(|i| i.kind == ItemKind::Mcp && i.name == "shared"),
            "fallback hidden once primary servers exist"
        );
    }

    #[test]
    fn zcode_stashed_servers_stay_listed_as_disabled() {
        let root = temp_dir("zcode-stash");
        let zc = root.join(".zcode");
        std::fs::create_dir_all(&zc).unwrap();
        std::fs::write(
            zc.join("config.json"),
            r#"{"mcp":{"servers":{"live":{"command":"live-mcp"}}},"_disabled_mcp_servers":{"parked":{"command":"parked-mcp"}}}"#,
        )
        .unwrap();

        let items = scan_provider(ProviderId::Zcode, &root, Scope::Project);
        let mcps: Vec<_> = items.iter().filter(|i| i.kind == ItemKind::Mcp).collect();
        assert_eq!(mcps.len(), 2, "stashed server stays listed");
        let live = mcps.iter().find(|i| i.name == "live").unwrap();
        assert_eq!(live.state, ItemState::Enabled);
        let parked = mcps.iter().find(|i| i.name == "parked").unwrap();
        assert_eq!(parked.state, ItemState::Disabled);

        assert!(
            !items
                .iter()
                .any(|i| i.kind == ItemKind::Hook && i.name == "shared"),
            "no fallback leakage"
        );
    }

    #[test]
    fn zcode_disabled_entry_flag_is_reported() {
        let root = temp_dir("zcode-flag");
        let zc = root.join(".zcode");
        std::fs::create_dir_all(&zc).unwrap();
        std::fs::write(
            zc.join("config.json"),
            r#"{"hooks":{"enabled":true,"events":{"Stop":[
                {"matcher":"*","enabled":false,"hooks":[{"type":"command","command":"slow"}]}
            ]}}}"#,
        )
        .unwrap();

        let items = scan_provider(ProviderId::Zcode, &root, Scope::Project);
        let hook = items
            .iter()
            .find(|i| i.kind == ItemKind::Hook)
            .expect("hook listed");
        assert_eq!(hook.state, ItemState::Disabled, "enabled:false is honored");
    }
}
