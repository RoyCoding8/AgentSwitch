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
        ProviderId::Junie => scan_junie(root, scope),
        ProviderId::Muse => scan_muse(root, scope),
        ProviderId::Grok => scan_grok(root, scope),
    })
}

pub fn provider_exists(id: ProviderId, root: &Path, scope: Scope) -> bool {
    let configured = provider::provider_dir(id, root, scope).is_ok_and(|path| path.is_dir());
    let shared_project_path = scope == Scope::Project
        && matches!(
            id,
            ProviderId::Codex
                | ProviderId::Antigravity
                | ProviderId::Zcode
                | ProviderId::OpenCode
                | ProviderId::Muse
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

fn collect_dir(
    dir: &Path,
    kind: ItemKind,
    provider: ProviderId,
    keep: impl Fn(&Path, &str) -> bool,
) -> Vec<ConfigItem> {
    let mut out = vec![];
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            let name = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            if keep(&p, &name) {
                out.push(ConfigItem::new(name, kind, p, provider));
            }
        }
    }
    out
}

fn collect_md(dir: &Path, kind: ItemKind, provider: ProviderId) -> Vec<ConfigItem> {
    collect_dir(dir, kind, provider, |p, name| {
        p.is_file() && (name.ends_with(".md") || name.ends_with(".md.disabled"))
    })
}

fn collect_subdirs(dir: &Path, kind: ItemKind, provider: ProviderId) -> Vec<ConfigItem> {
    collect_dir(dir, kind, provider, |p, _| p.is_dir())
}

fn disabled_dir(dir: &Path) -> PathBuf {
    let mut dis_dir = dir.to_path_buf();
    dis_dir.set_file_name(format!(
        "{}.disabled",
        dir.file_name().unwrap_or_default().to_string_lossy()
    ));
    dis_dir
}

fn collect_md_both(dir: &Path, kind: ItemKind, provider: ProviderId) -> Vec<ConfigItem> {
    let mut out = collect_md(dir, kind, provider);
    out.extend(collect_md(&disabled_dir(dir), kind, provider));
    out
}

fn collect_subdirs_both(dir: &Path, kind: ItemKind, provider: ProviderId) -> Vec<ConfigItem> {
    let mut out = collect_subdirs(dir, kind, provider);
    out.extend(collect_subdirs(&disabled_dir(dir), kind, provider));
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

fn instruction_files(
    scope: Scope,
    root: &Path,
    d: &Path,
    names: &[&str],
    provider: ProviderId,
) -> Vec<ConfigItem> {
    let base = if scope == Scope::Project { root } else { d };
    let mut out = vec![];
    for name in names {
        out.extend(check_file(
            base.join(name),
            ItemKind::InstructionFile,
            provider,
        ));
    }
    out
}

fn shared_skills_items(scope: Scope, root: &Path, provider: ProviderId) -> Vec<ConfigItem> {
    let dir = if scope == Scope::Project {
        root.join(".agents").join("skills")
    } else {
        provider::home_dir()
            .map(|home| home.join(".agents").join("skills"))
            .unwrap_or_default()
    };
    if dir.as_os_str().is_empty() {
        return vec![];
    }
    collect_subdirs_both(&dir, ItemKind::Skill, provider)
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
    let text = std::fs::read_to_string(path).ok()?;
    crate::config_store::parse_json(path, &text).ok()
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
            let opencode_mcp = provider == ProviderId::OpenCode && key == "mcp";
            let nested = obj
                .get("servers")
                .and_then(|value| value.as_object())
                .filter(|servers| {
                    opencode_mcp
                        && !["type", "enabled"]
                            .iter()
                            .any(|key| servers.get(*key).is_some_and(|value| !value.is_object()))
                });
            let global_timeout = opencode_mcp
                && obj
                    .get("timeout")
                    .and_then(|value| value.as_object())
                    .is_some_and(|timeout| {
                        !["type", "enabled"]
                            .iter()
                            .any(|key| timeout.get(*key).is_some_and(|value| !value.is_object()))
                            && (timeout.is_empty()
                                || ["startup", "catalog", "execution"]
                                    .iter()
                                    .any(|key| timeout.contains_key(*key)))
                            && ["startup", "catalog", "execution"].iter().all(|key| {
                                timeout.get(*key).is_none_or(|value| {
                                    value.as_f64().is_some_and(|n| n > 0.0 && n.fract() == 0.0)
                                })
                            })
                    });
            let mut entries: Vec<_> = obj
                .iter()
                .filter(|(name, _)| nested.is_none() || name.as_str() != "servers")
                .filter(|(name, _)| !global_timeout || name.as_str() != "timeout")
                .map(|(name, value)| (key, name, value))
                .collect();
            if let Some(servers) = nested {
                entries.extend(
                    servers
                        .iter()
                        .filter(|(name, _)| {
                            name.as_str() == "servers"
                                || (global_timeout && name.as_str() == "timeout")
                                || !obj.contains_key(*name)
                        })
                        .map(|(name, value)| ("mcp/servers", name, value)),
                );
            }
            for (section, name, value) in entries {
                if opencode_mcp
                    && !(value.get("type").is_some_and(|v| v.is_string())
                        || value.get("enabled").is_some_and(|v| v.is_boolean())
                        || value.get("disabled").is_some_and(|v| v.is_boolean()))
                {
                    continue;
                }
                let mut item = ConfigItem::new(name.clone(), kind, path.to_owned(), provider);
                item.state = if base_state == ItemState::Disabled
                    || if opencode_mcp {
                        value
                            .get("enabled")
                            .and_then(|v| v.as_bool())
                            .map(|enabled| !enabled)
                            .unwrap_or_else(|| {
                                value.get("disabled").and_then(|v| v.as_bool()) == Some(true)
                            })
                    } else if provider == ProviderId::OpenCode && key == "agent" {
                        value.get("disable").and_then(|v| v.as_bool()) == Some(true)
                    } else {
                        value.get("disabled").and_then(|v| v.as_bool()) == Some(true)
                            || value.get("enabled").and_then(|v| v.as_bool()) == Some(false)
                    } {
                    ItemState::Disabled
                } else {
                    ItemState::Enabled
                };
                item.editable = false;
                item.toggle_spec = Some(match (provider, key) {
                    (ProviderId::OpenCode, "agent") => ToggleSpec::JsonFlag {
                        section: key.to_string(),
                        name: name.clone(),
                        flag: "disable".into(),
                        enabled_value: false,
                        disabled_value: true,
                    },
                    (ProviderId::OpenCode, "mcp") => {
                        let uses_disabled = value.get("enabled").is_none()
                            && (section == "mcp/servers" || value.get("disabled").is_some());
                        ToggleSpec::JsonFlag {
                            section: section.to_string(),
                            name: name.clone(),
                            flag: if uses_disabled { "disabled" } else { "enabled" }.into(),
                            enabled_value: !uses_disabled,
                            disabled_value: uses_disabled,
                        }
                    }
                    (ProviderId::Muse, "mcp_servers") => ToggleSpec::JsonFlag {
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

fn hook_names(event: &str, entry: &serde_json::Value) -> (String, String) {
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
    (display, hook_name.unwrap_or_else(|| matcher.to_string()))
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
        let Some(arr) = entries.as_array() else {
            continue;
        };
        for (order, entry) in arr.iter().enumerate() {
            let json_entry = toml_to_json(entry);
            let (display, hook_name) = hook_names(event, &json_entry);
            let loc = HookLoc {
                section: "hooks".into(),
                event: event.clone(),
                order,
                hook_name,
                fingerprint: json_fingerprint(&json_entry),
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
            let (display, hook_name) = hook_names(&event, entry);
            let entry_flag_disabled = provider == ProviderId::Zcode
                && entry.get("enabled").and_then(|v| v.as_bool()) == Some(false);
            let is_disabled = force_disabled || entry_flag_disabled;
            let loc = HookLoc {
                section: section_path.to_string(),
                event: format!("{}{}", event_prefix, event),
                order,
                hook_name,
                fingerprint: if event_prefix == "_stashed_" {
                    crate::toggler::stash_entry_fingerprint(entry)
                } else if provider == ProviderId::Zcode {
                    crate::toggler::zcode_entry_fingerprint(entry)
                } else {
                    json_fingerprint(entry)
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
    let mut items = instruction_files(scope, root, &d, &["CLAUDE.md"], ProviderId::Claude);
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
    for settings_name in ["settings.json", "settings.local.json"] {
        items.extend(scan_hook_entries(
            &d.join(settings_name),
            ProviderId::Claude,
            "hooks",
            false,
        ));
    }
    let mcp_path = match scope {
        Scope::Project => root.join(".mcp.json"),
        Scope::Global => provider::home_dir()
            .map(|home| home.join(".claude.json"))
            .unwrap_or_default(),
    };
    let mut mcp_items = scan_json_keys(&mcp_path, "mcpServers", ItemKind::Mcp, ProviderId::Claude);
    if scope == Scope::Project {
        let stash_disabled: std::collections::HashSet<String> = read_json(&mcp_path)
            .and_then(|doc| {
                doc.get("_disabled_mcpServers")
                    .and_then(|v| v.as_object())
                    .map(|obj| obj.keys().cloned().collect())
            })
            .unwrap_or_default();
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
            if stash_disabled.contains(&item.name) {
                continue;
            }
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
    let mut items = instruction_files(scope, root, &d, &["AGENTS.md"], ProviderId::Codex);
    if scope == Scope::Project {
        items.extend(shared_skills_items(scope, root, ProviderId::Codex));
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
        items.extend(scan_hook_entries(&hooks, ProviderId::Codex, "hooks", false));
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
    items.extend(instruction_files(
        scope,
        root,
        &d,
        &["GEMINI.md", "AGENTS.md"],
        ProviderId::Antigravity,
    ));
    if scope == Scope::Project {
        items.extend(shared_skills_items(scope, root, ProviderId::Antigravity));
    } else {
        items.extend(collect_subdirs_both(
            &d.join("skills"),
            ItemKind::Skill,
            ProviderId::Antigravity,
        ));
    }
    let mcp_path =
        if scope == Scope::Global && crate::provider::env_path("ANTIGRAVITY_HOME").is_none() {
            let Ok(home) = crate::provider::home_dir() else {
                return items;
            };
            home.join(".gemini").join("config").join("mcp_config.json")
        } else {
            d.join("mcp_config.json")
        };
    items.extend(scan_json_keys(
        &mcp_path,
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
        items.extend(collect_hook_files(
            &agents_dir,
            ProviderId::Kiro,
            force_disabled,
        ));
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
    let mut items = instruction_files(scope, root, &d, &["AGENTS.md"], ProviderId::OpenCode);
    items.extend(collect_subdirs_both(
        &d.join("skills"),
        ItemKind::Skill,
        ProviderId::OpenCode,
    ));
    if scope == Scope::Project {
        items.extend(shared_skills_items(scope, root, ProviderId::OpenCode));
        items.extend(collect_subdirs_both(
            &root.join(".claude").join("skills"),
            ItemKind::Skill,
            ProviderId::OpenCode,
        ));
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
        if !flat.exists()
            && !flat.with_extension("jsonc").exists()
            && (nested.exists() || nested.with_extension("jsonc").exists())
        {
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
    if let Some(doc) = read_json(&actual_cfg) {
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
    items
}

fn scan_zcode(root: &Path, scope: Scope) -> Vec<ConfigItem> {
    let Ok(d) = provider_dir(ProviderId::Zcode, root, scope) else {
        return vec![];
    };
    let mut items = instruction_files(scope, root, &d, &["AGENTS.md"], ProviderId::Zcode);
    items.extend(collect_subdirs_both(
        &d.join("skills"),
        ItemKind::Skill,
        ProviderId::Zcode,
    ));
    items.extend(shared_skills_items(scope, root, ProviderId::Zcode));

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
            false,
        ));
    }
    items
}

fn scan_junie(root: &Path, scope: Scope) -> Vec<ConfigItem> {
    let Ok(d) = provider_dir(ProviderId::Junie, root, scope) else {
        return vec![];
    };
    let mut items = vec![];
    if scope == Scope::Project {
        items.extend(check_file(
            d.join("AGENTS.md"),
            ItemKind::InstructionFile,
            ProviderId::Junie,
        ));
        items.extend(check_file(
            root.join("AGENTS.md"),
            ItemKind::InstructionFile,
            ProviderId::Junie,
        ));
        items.extend(check_file(
            d.join("playbook.md"),
            ItemKind::InstructionFile,
            ProviderId::Junie,
        ));
        items.extend(check_file(
            d.join("guidelines.md"),
            ItemKind::InstructionFile,
            ProviderId::Junie,
        ));
        items.extend(collect_md_both(
            &d.join("rules"),
            ItemKind::Rule,
            ProviderId::Junie,
        ));
        items.extend(collect_md_both(
            &d.join("guidelines"),
            ItemKind::Rule,
            ProviderId::Junie,
        ));
    }
    items.extend(collect_md_both(
        &d.join("commands"),
        ItemKind::Skill,
        ProviderId::Junie,
    ));
    items.extend(collect_subdirs_both(
        &d.join("skills"),
        ItemKind::Skill,
        ProviderId::Junie,
    ));
    items.extend(shared_skills_items(scope, root, ProviderId::Junie));
    items.extend(scan_json_keys(
        &d.join("mcp").join("mcp.json"),
        "mcpServers",
        ItemKind::Mcp,
        ProviderId::Junie,
    ));
    if scope == Scope::Global {
        items.extend(scan_hook_entries(
            &d.join("config.json"),
            ProviderId::Junie,
            "hooks",
            false,
        ));
    }
    items
}

fn scan_muse(root: &Path, scope: Scope) -> Vec<ConfigItem> {
    let Ok(d) = provider_dir(ProviderId::Muse, root, scope) else {
        return vec![];
    };
    let mut items = vec![];
    if scope == Scope::Project {
        items.extend(check_file(
            root.join("AGENTS.md"),
            ItemKind::InstructionFile,
            ProviderId::Muse,
        ));
        items.extend(check_file(
            root.join(".agents").join("AGENTS.md"),
            ItemKind::InstructionFile,
            ProviderId::Muse,
        ));
        items.extend(scan_hook_entries(
            &root.join(".muse").join("hooks.json"),
            ProviderId::Muse,
            "hooks",
            false,
        ));
    } else {
        items.extend(check_file(
            d.join("AGENTS.md"),
            ItemKind::InstructionFile,
            ProviderId::Muse,
        ));
        items.extend(collect_subdirs_both(
            &d.join("skills"),
            ItemKind::Skill,
            ProviderId::Muse,
        ));
        items.extend(scan_json_keys(
            &d.join("settings.json"),
            "mcp_servers",
            ItemKind::Mcp,
            ProviderId::Muse,
        ));
        items.extend(scan_hook_entries(
            &d.join("settings.json"),
            ProviderId::Muse,
            "hooks",
            false,
        ));
    }
    items.extend(shared_skills_items(scope, root, ProviderId::Muse));
    items
}

fn scan_grok(root: &Path, scope: Scope) -> Vec<ConfigItem> {
    let Ok(d) = provider_dir(ProviderId::Grok, root, scope) else {
        return vec![];
    };
    let mut items = instruction_files(scope, root, &d, &["AGENTS.md"], ProviderId::Grok);
    items.extend(collect_md_both(
        &d.join("rules"),
        ItemKind::Rule,
        ProviderId::Grok,
    ));
    items.extend(collect_subdirs_both(
        &d.join("skills"),
        ItemKind::Skill,
        ProviderId::Grok,
    ));
    items.extend(scan_toml_mcp(&d.join("config.toml"), ProviderId::Grok));
    items.extend(collect_hook_files(
        &d.join("hooks"),
        ProviderId::Grok,
        false,
    ));
    items
}

fn collect_hook_files(dir: &Path, provider: ProviderId, force_disabled: bool) -> Vec<ConfigItem> {
    let mut out = vec![];
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_file() && p.extension().and_then(|ext| ext.to_str()) == Some("json") {
            out.extend(scan_hook_entries(&p, provider, "hooks", force_disabled));
        }
    }
    out
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

    #[test]
    fn stash_disabled_project_mcp_restores_through_the_stash_spec() {
        let root = crate::test_env::temp_dir("scanner-claude-stash-mcp");
        std::fs::write(
            root.join(".mcp.json"),
            r#"{"_disabled_mcpServers":{"docs":{"command":"ctx"}}}"#,
        )
        .unwrap();
        std::fs::create_dir_all(root.join(".claude")).unwrap();

        let items = scan_provider(ProviderId::Claude, &root, Scope::Project);
        let docs = items
            .iter()
            .find(|item| item.kind == ItemKind::Mcp && item.name == "docs")
            .unwrap();
        assert_eq!(docs.state, ItemState::Disabled);
        assert!(
            matches!(docs.toggle_spec, Some(ToggleSpec::JsonStash { .. })),
            "stash-parked servers must toggle through the stash, not the approval lists"
        );
        let mut docs = docs.clone();
        crate::toggler::toggle_item(&mut docs).unwrap();
        let mcp: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(root.join(".mcp.json")).unwrap())
                .unwrap();
        assert_eq!(mcp["mcpServers"]["docs"]["command"], "ctx");
        assert!(mcp.get("_disabled_mcpServers").is_none());
    }

    #[test]
    fn opencode_agent_toggle_uses_disable_and_rescans() {
        for extension in ["json", "jsonc"] {
            let root = crate::test_env::temp_dir("scanner-opencode-agent");
            let path = root.join(format!("opencode.{extension}"));
            std::fs::write(
                &path,
                r#"{"agent":{"review":{"disable":true,"prompt":"Review changes"}},"mcp":{"docs":{"enabled":true,"type":"remote","url":"https://example.test"}}}"#,
            )
            .unwrap();
            let mut agent = scan_provider(ProviderId::OpenCode, &root, Scope::Project)
                .into_iter()
                .find(|item| item.name == "review")
                .unwrap();
            assert_eq!(agent.state, ItemState::Disabled);
            crate::toggler::toggle_item(&mut agent).unwrap();
            let enabled = read_json(&path).unwrap();
            assert_eq!(
                enabled["agent"]["review"],
                serde_json::json!({"disable": false, "prompt": "Review changes"})
            );
            assert_eq!(enabled["mcp"]["docs"]["enabled"], true);
            let mut agent = scan_provider(ProviderId::OpenCode, &root, Scope::Project)
                .into_iter()
                .find(|item| item.name == "review")
                .unwrap();
            assert_eq!(agent.state, ItemState::Enabled);
            crate::toggler::toggle_item(&mut agent).unwrap();
            assert_eq!(
                read_json(&path).unwrap()["agent"]["review"],
                serde_json::json!({"disable": true, "prompt": "Review changes"})
            );
            let agent = scan_provider(ProviderId::OpenCode, &root, Scope::Project)
                .into_iter()
                .find(|item| item.name == "review")
                .unwrap();
            assert_eq!(agent.state, ItemState::Disabled);
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn opencode_nested_jsonc_scans_mcp() {
        let root = crate::test_env::temp_dir("scanner-opencode-jsonc");
        std::fs::create_dir_all(root.join(".opencode")).unwrap();
        let path = root.join(".opencode/opencode.jsonc");
        std::fs::write(
            &path,
            r#"{
            // Local server
            "mcp": {"audit-jsonc": {
                "type": "local", "command": ["audit-never-execute"],
                /* Keep enabled */ "enabled": true,
            },},
        }"#,
        )
        .unwrap();
        let items = scan_provider(ProviderId::OpenCode, &root, Scope::Project);
        let item = items
            .iter()
            .find(|item| item.kind == ItemKind::Mcp && item.name == "audit-jsonc")
            .expect("nested JSONC MCP server must be discovered");
        assert_eq!(item.path, path);
        assert_eq!(item.state, ItemState::Enabled);
        let detail: serde_json::Value =
            serde_json::from_str(item.detail.as_deref().unwrap()).unwrap();
        assert_eq!(
            detail["command"],
            serde_json::json!(["audit-never-execute"])
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn opencode_mcp_envelope_toggle_roundtrip() {
        let root = crate::test_env::temp_dir("scanner-opencode-envelope");
        let path = root.join("opencode.jsonc");
        let original = r#"{
  "mcp": {
    "timeout": {"catalog": 5000, "execution": 5000},
    "flat": {"type": "remote", "url": "https://example.test", "enabled": true},
    "servers": {
      // Preserve nested settings
      "docs": {"type": "local", "command": ["never-execute"], "disabled": true},
      "flat": {"type": "remote", "url": "https://shadow.test", "disabled": true}
    }
  }
}"#;
        std::fs::write(&path, original).unwrap();
        let items = scan_provider(ProviderId::OpenCode, &root, Scope::Project);
        let mcps: Vec<_> = items
            .into_iter()
            .filter(|item| item.kind == ItemKind::Mcp)
            .collect();
        assert_eq!(mcps.len(), 2);
        assert!(mcps
            .iter()
            .any(|item| item.name == "flat" && item.state == ItemState::Enabled));
        let mut docs = mcps
            .into_iter()
            .find(|item| item.name == "docs")
            .expect("nested MCP server must be discovered");
        assert_eq!(docs.state, ItemState::Disabled);
        crate::toggler::toggle_item(&mut docs).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            original.replacen("\"disabled\": true", "\"disabled\": false", 1)
        );
        let mut docs = scan_provider(ProviderId::OpenCode, &root, Scope::Project)
            .into_iter()
            .find(|item| item.name == "docs")
            .unwrap();
        assert_eq!(docs.state, ItemState::Enabled);
        crate::toggler::toggle_item(&mut docs).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn opencode_mcp_server_names_and_flags_roundtrip() {
        let root = crate::test_env::temp_dir("scanner-opencode-mcp-flags");
        let path = root.join("opencode.json");
        for (original, name, section, flag, initially_enabled) in [
            (
                serde_json::json!({"mcp": {"timeout": {"catalog": 5000, "execution": 5000}, "servers": {"timeout": {"type": "remote", "url": "https://example.test", "disabled": true}}}}),
                "timeout",
                "mcp/servers",
                "disabled",
                false,
            ),
            (
                serde_json::json!({"mcp": {"servers": {"servers": {"type": "remote", "url": "https://example.test", "disabled": false}}}}),
                "servers",
                "mcp/servers",
                "disabled",
                true,
            ),
            (
                serde_json::json!({"mcp": {"servers": {"type": "remote", "url": "https://example.test", "enabled": true}}}),
                "servers",
                "mcp",
                "enabled",
                true,
            ),
            (
                serde_json::json!({"mcp": {"timeout": {"type": "remote", "url": "https://example.test", "enabled": false}}}),
                "timeout",
                "mcp",
                "enabled",
                false,
            ),
            (
                serde_json::json!({"mcp": {"servers": {"enabled": {"type": "remote", "url": "https://example.test", "enabled": false}}}}),
                "enabled",
                "mcp/servers",
                "enabled",
                false,
            ),
            (
                serde_json::json!({"mcp": {"servers": {"type": {"type": "remote", "url": "https://example.test", "disabled": false}}}}),
                "type",
                "mcp/servers",
                "disabled",
                true,
            ),
            (
                serde_json::json!({"mcp": {"docs": {"type": "remote", "url": "https://example.test", "disabled": true}}}),
                "docs",
                "mcp",
                "disabled",
                false,
            ),
        ] {
            std::fs::write(&path, serde_json::to_string(&original).unwrap()).unwrap();
            let mut items: Vec<_> = scan_provider(ProviderId::OpenCode, &root, Scope::Project)
                .into_iter()
                .filter(|item| item.kind == ItemKind::Mcp)
                .collect();
            assert_eq!(items.len(), 1, "{original}");
            let item = &mut items[0];
            assert_eq!(item.name, name);
            assert_eq!(item.state.is_enabled(), initially_enabled);
            crate::toggler::toggle_item(item).unwrap();
            let mut expected = original.clone();
            let mut entry = &mut expected;
            for key in section.split('/') {
                entry = &mut entry[key];
            }
            entry[name][flag] = serde_json::json!(if flag == "enabled" {
                !initially_enabled
            } else {
                initially_enabled
            });
            assert_eq!(read_json(&path).unwrap(), expected);
            let mut rescanned = scan_provider(ProviderId::OpenCode, &root, Scope::Project)
                .into_iter()
                .find(|item| item.kind == ItemKind::Mcp)
                .unwrap();
            assert_eq!(rescanned.state.is_enabled(), !initially_enabled);
            crate::toggler::toggle_item(&mut rescanned).unwrap();
            assert_eq!(read_json(&path).unwrap(), original);
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn opencode_flat_jsonc_toggle_preserves_comments() {
        let root = crate::test_env::temp_dir("scanner-opencode-jsonc-toggle");
        let path = root.join("opencode.jsonc");
        let original = r#"{
  // Keep this URL and comment
  "mcp": {"docs": {"type": "remote", "url": "https://example.test/a//b/*c*/", "enabled": true,},},
  /* Keep plugin options */ "plugin": [["audit-plugin", {"option": true,}],],
}"#;
        std::fs::write(&path, original).unwrap();
        let items = scan_provider(ProviderId::OpenCode, &root, Scope::Project);
        let mut item = items
            .iter()
            .find(|item| item.kind == ItemKind::Mcp && item.name == "docs")
            .unwrap()
            .clone();
        let plugin = items
            .iter()
            .find(|item| item.kind == ItemKind::Plugin)
            .unwrap();
        assert_eq!(plugin.name, "audit-plugin");
        crate::toggler::toggle_item(&mut item).unwrap();
        assert_eq!(item.state, ItemState::Disabled);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            original.replacen("\"enabled\": true", "\"enabled\": false", 1)
        );
        assert_eq!(
            std::fs::read_to_string(path.with_extension("jsonc.bak")).unwrap(),
            original
        );
        let mut rescanned = scan_provider(ProviderId::OpenCode, &root, Scope::Project)
            .into_iter()
            .find(|item| item.kind == ItemKind::Mcp)
            .unwrap();
        assert_eq!(rescanned.state, ItemState::Disabled);
        crate::toggler::toggle_item(&mut rescanned).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn claude_hooks_in_settings_local_json_are_scanned() {
        let root = crate::test_env::temp_dir("scanner-claude-local-hooks");
        let claude = root.join(".claude");
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::write(
            claude.join("settings.local.json"),
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"local-guard"}]}]}}"#,
        )
        .unwrap();

        let items = scan_provider(ProviderId::Claude, &root, Scope::Project);
        assert!(
            items
                .iter()
                .any(|item| item.kind == ItemKind::Hook && item.name == "local-guard"),
            "hooks declared in settings.local.json must appear in the scan"
        );
    }

    #[test]
    fn claude_project_mcp_comes_from_dot_mcp_json() {
        let root = crate::test_env::temp_dir("scanner-claude-mcp");
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
    fn antigravity_mcp_paths_follow_scope_and_override() {
        let home = crate::test_env::temp_dir("scanner-antigravity-mcp");
        let root = home.join("workspace");
        let custom = home.join("custom");
        let global = home.join(".gemini/config/mcp_config.json");
        let project = root.join(".agents/mcp_config.json");
        let overridden = custom.join("mcp_config.json");
        for (path, name) in [
            (&global, "global"),
            (&project, "project"),
            (&overridden, "custom"),
        ] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(
                path,
                format!(r#"{{"mcpServers":{{"{name}":{{"serverUrl":"https://example.test"}}}}}}"#),
            )
            .unwrap();
        }
        for (scope, override_dir, expected_path, expected_name) in [
            (Scope::Global, Path::new(""), &global, "global"),
            (Scope::Project, Path::new(""), &project, "project"),
            (Scope::Global, custom.as_path(), &overridden, "custom"),
            (Scope::Project, custom.as_path(), &project, "project"),
        ] {
            crate::test_env::with_env_vars(
                &[
                    ("AGENT_SWITCH_HOME", &home),
                    ("ANTIGRAVITY_HOME", override_dir),
                ],
                || {
                    let items = scan_provider(ProviderId::Antigravity, &root, scope);
                    let mcps: Vec<_> = items
                        .iter()
                        .filter(|item| item.kind == ItemKind::Mcp)
                        .collect();
                    assert_eq!(mcps.len(), 1);
                    assert_eq!(mcps[0].name, expected_name);
                    assert_eq!(&mcps[0].path, expected_path);
                },
            );
        }
    }

    #[test]
    fn antigravity_scans_project_hooks() {
        let root = crate::test_env::temp_dir("scanner-antigravity-project");
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
        let root = crate::test_env::temp_dir("scanner-antigravity-state");
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
        let root = crate::test_env::temp_dir("scanner-antigravity-roundtrip");
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
        let root = crate::test_env::temp_dir("scanner-claude-roundtrip");
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
    fn hook_order_survives_multiple_toggles_with_rescans() {
        let root = crate::test_env::temp_dir("scanner-hook-order");
        let claude = root.join(".claude");
        std::fs::create_dir_all(&claude).unwrap();
        let settings = claude.join("settings.json");
        let entries: Vec<_> = ["a", "b", "c", "d"]
            .into_iter()
            .map(|command| serde_json::json!({"hooks":[{"command":command}]}))
            .collect();
        std::fs::write(
            &settings,
            serde_json::json!({"hooks":{"Stop":entries}}).to_string(),
        )
        .unwrap();

        for name in ["a", "c", "c", "a"] {
            let mut item = scan_provider(ProviderId::Claude, &root, Scope::Project)
                .into_iter()
                .find(|item| item.kind == ItemKind::Hook && item.name == name)
                .unwrap();
            crate::toggler::toggle_item(&mut item).unwrap();
        }
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        let commands: Vec<_> = doc["hooks"]["Stop"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["hooks"][0]["command"].as_str().unwrap())
            .collect();
        assert_eq!(commands, ["a", "b", "c", "d"]);
    }

    #[test]
    fn hook_order_survives_all_three_hook_disable_and_restore_orders() {
        let orders = [
            ["a", "b", "c"],
            ["a", "c", "b"],
            ["b", "a", "c"],
            ["b", "c", "a"],
            ["c", "a", "b"],
            ["c", "b", "a"],
        ];
        let root = crate::test_env::temp_dir("scanner-hook-permutations");
        let claude = root.join(".claude");
        std::fs::create_dir_all(&claude).unwrap();
        let settings = claude.join("settings.json");
        let original = serde_json::json!({"hooks":{"Stop":[
            {"hooks":[{"command":"a"}]},
            {"hooks":[{"command":"b"}]},
            {"hooks":[{"command":"c"}]},
            {"hooks":[{"command":"d"}]}
        ]}});
        for disabled in orders {
            for restored in orders {
                std::fs::write(&settings, original.to_string()).unwrap();
                for name in disabled.into_iter().chain(restored) {
                    let mut item = scan_provider(ProviderId::Claude, &root, Scope::Project)
                        .into_iter()
                        .find(|item| item.kind == ItemKind::Hook && item.name == name)
                        .unwrap();
                    crate::toggler::toggle_item(&mut item).unwrap();
                }
                let actual: serde_json::Value =
                    serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
                assert_eq!(
                    actual, original,
                    "disable {disabled:?}, restore {restored:?}"
                );
                assert!(!sidecar_path_exists(&settings));
            }
        }
    }

    #[test]
    fn identical_hooks_can_be_disabled_and_restored_after_rescanning() {
        let root = crate::test_env::temp_dir("scanner-identical-hooks");
        let claude = root.join(".claude");
        std::fs::create_dir_all(&claude).unwrap();
        let settings = claude.join("settings.json");
        let original = serde_json::json!({"hooks":{"Stop":[
            {"hooks":[{"command":"same"}]},
            {"hooks":[{"command":"same"}]}
        ]}});
        std::fs::write(&settings, original.to_string()).unwrap();
        for state in [
            ItemState::Enabled,
            ItemState::Enabled,
            ItemState::Disabled,
            ItemState::Disabled,
        ] {
            let mut item = scan_provider(ProviderId::Claude, &root, Scope::Project)
                .into_iter()
                .find(|item| item.kind == ItemKind::Hook && item.state == state)
                .unwrap();
            crate::toggler::toggle_item(&mut item).unwrap();
        }
        let actual: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(actual, original);
        assert!(!sidecar_path_exists(&settings));
    }

    #[test]
    fn codex_hooks_json_disable_rescan_enable_round_trip() {
        let root = crate::test_env::temp_dir("scanner-codex-roundtrip");
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
        let root = crate::test_env::temp_dir("scanner-kiro-roundtrip");
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
        let root = crate::test_env::temp_dir("scanner-legacy-roundtrip");
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
        let root = crate::test_env::temp_dir("scanner-hook-dedup");
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
        let root = crate::test_env::temp_dir("scanner-zcode-scan");
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
        let root = crate::test_env::temp_dir("scanner-zcode-fallback");
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
        let root = crate::test_env::temp_dir("scanner-zcode-stash");
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
    fn junie_scans_skills_commands_rules_mcp_and_user_hooks() {
        let root = crate::test_env::temp_dir("scanner-junie-scan");
        let junie = root.join(".junie");
        std::fs::create_dir_all(junie.join("skills").join("review")).unwrap();
        std::fs::create_dir_all(junie.join("commands")).unwrap();
        std::fs::create_dir_all(junie.join("rules")).unwrap();
        std::fs::create_dir_all(junie.join("mcp")).unwrap();
        std::fs::write(junie.join("AGENTS.md"), "guidelines").unwrap();
        std::fs::write(junie.join("commands").join("deploy.md"), "deploy prompt").unwrap();
        std::fs::write(junie.join("rules").join("style.md"), "style rules").unwrap();
        std::fs::write(
            junie.join("mcp").join("mcp.json"),
            r#"{"mcpServers":{"ctx":{"command":"npx","args":["-y","ctx"]}}}"#,
        )
        .unwrap();

        let items = scan_provider(ProviderId::Junie, &root, Scope::Project);
        assert!(items
            .iter()
            .any(|i| i.kind == ItemKind::InstructionFile && i.name == "AGENTS.md"));
        assert!(items
            .iter()
            .any(|i| i.kind == ItemKind::Skill && i.name == "review"));
        assert!(items
            .iter()
            .any(|i| i.kind == ItemKind::Skill && i.name == "deploy.md"));
        assert!(items
            .iter()
            .any(|i| i.kind == ItemKind::Rule && i.name == "style.md"));
        let ctx = items
            .iter()
            .find(|i| i.kind == ItemKind::Mcp && i.name == "ctx")
            .unwrap();
        assert_eq!(ctx.state, ItemState::Enabled);
        assert!(matches!(
            ctx.toggle_spec,
            Some(ToggleSpec::JsonStash { .. })
        ));

        let global_items = scan_provider(ProviderId::Junie, &root, Scope::Global);
        assert!(
            !global_items
                .iter()
                .any(|i| i.kind == ItemKind::InstructionFile),
            "Junie documents no global guideline files"
        );
    }

    #[test]
    fn junie_project_hooks_are_not_scanned_because_junie_ignores_them() {
        let root = crate::test_env::temp_dir("scanner-junie-project-hooks");
        let junie = root.join(".junie");
        std::fs::create_dir_all(&junie).unwrap();
        std::fs::write(
            junie.join("config.json"),
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"repo-hook"}]}]}}"#,
        )
        .unwrap();

        let items = scan_provider(ProviderId::Junie, &root, Scope::Project);
        assert!(
            !items.iter().any(|i| i.kind == ItemKind::Hook),
            "project-local Junie hooks are ignored by the CLI, so they must not be listed"
        );
    }

    #[test]
    fn junie_user_hooks_toggle_through_the_stash() {
        let root = crate::test_env::temp_dir("scanner-junie-user-hooks");
        let home = root.join("home");
        let junie = home.join(".junie");
        std::fs::create_dir_all(&junie).unwrap();
        std::fs::write(
            junie.join("config.json"),
            r#"{"hooks":{"SessionStart":[{"matcher":"startup","hooks":[{"type":"command","command":"aws sso login"}]}]}}"#,
        )
        .unwrap();

        crate::test_env::with_env_vars(&[("AGENT_SWITCH_HOME", &home)], || {
            std::fs::create_dir_all(junie.join("commands")).unwrap();
            std::fs::write(junie.join("commands").join("tide.md"), "global command").unwrap();
            let items = scan_provider(ProviderId::Junie, &root, Scope::Global);
            assert!(
                items
                    .iter()
                    .any(|i| i.kind == ItemKind::Skill && i.name == "tide.md"),
                "user-scope ~/.junie/commands must be scanned"
            );
            let hook = items
                .iter()
                .find(|i| i.kind == ItemKind::Hook && i.name == "aws sso login")
                .unwrap()
                .clone();
            assert_eq!(hook.state, ItemState::Enabled);
            let mut hook = hook;
            crate::toggler::toggle_item(&mut hook).unwrap();
            assert_eq!(hook.state, ItemState::Disabled);
            let doc: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(junie.join("config.json")).unwrap())
                    .unwrap();
            assert!(
                doc.pointer("/hooks/SessionStart").is_none(),
                "hook entry must be stashed away"
            );
            let stash: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(junie.join("config.json.agentswitch")).unwrap(),
            )
            .unwrap();
            assert!(stash.get("SessionStart").is_some());
        });
    }

    #[test]
    fn muse_scans_skills_settings_mcp_project_hooks_and_instructions() {
        let root = crate::test_env::temp_dir("scanner-muse-scan");
        let agents = root.join(".agents");
        std::fs::create_dir_all(agents.join("skills").join("plan")).unwrap();
        std::fs::create_dir_all(root.join(".muse")).unwrap();
        std::fs::write(root.join("AGENTS.md"), "rules").unwrap();
        std::fs::write(
            root.join(".muse").join("hooks.json"),
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"guard"}]}]}}"#,
        )
        .unwrap();

        let items = scan_provider(ProviderId::Muse, &root, Scope::Project);
        assert!(items
            .iter()
            .any(|i| i.kind == ItemKind::InstructionFile && i.name == "AGENTS.md"));
        assert!(items
            .iter()
            .any(|i| i.kind == ItemKind::Skill && i.name == "plan"));
        assert!(items
            .iter()
            .any(|i| i.kind == ItemKind::Hook && i.name == "guard"));
    }

    #[test]
    fn muse_user_settings_mcp_servers_toggle_and_report_state() {
        let root = crate::test_env::temp_dir("scanner-muse-mcp");
        let home = root.join("home");
        let muse = home.join(".config").join("muse");
        std::fs::create_dir_all(&muse).unwrap();
        std::fs::write(
            muse.join("settings.json"),
            r#"{"schema_version":1,"mcp_servers":{"tools":{"transport":"stdio","command":"my-mcp","args":[],"enabled":true},"off":{"transport":"stdio","command":"x","args":[],"enabled":false}}}"#,
        )
        .unwrap();

        crate::test_env::with_env_vars(&[("AGENT_SWITCH_HOME", &home)], || {
            let items = scan_provider(ProviderId::Muse, &root, Scope::Global);
            let tools = items
                .iter()
                .find(|i| i.kind == ItemKind::Mcp && i.name == "tools")
                .unwrap()
                .clone();
            assert_eq!(tools.state, ItemState::Enabled);
            let off = items
                .iter()
                .find(|i| i.kind == ItemKind::Mcp && i.name == "off")
                .unwrap()
                .clone();
            assert_eq!(off.state, ItemState::Disabled);

            let mut tools = tools;
            crate::toggler::toggle_item(&mut tools).unwrap();
            assert_eq!(tools.state, ItemState::Disabled);

            let mut off = off;
            crate::toggler::toggle_item(&mut off).unwrap();
            assert_eq!(
                off.state,
                ItemState::Enabled,
                "vendor-disabled server re-enables in place"
            );
            let doc: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(muse.join("settings.json")).unwrap())
                    .unwrap();
            assert_eq!(
                doc.pointer("/mcp_servers/tools/enabled"),
                Some(&serde_json::json!(false))
            );
            assert_eq!(
                doc.pointer("/mcp_servers/off/enabled"),
                Some(&serde_json::json!(true))
            );
            assert!(doc.get("_disabled_mcp_servers").is_none());
        });
    }

    #[test]
    fn grok_scans_agents_md_rules_skills_toml_mcp_and_hook_files() {
        let root = crate::test_env::temp_dir("scanner-grok-scan");
        let grok = root.join(".grok");
        std::fs::create_dir_all(grok.join("rules")).unwrap();
        std::fs::create_dir_all(grok.join("skills").join("review")).unwrap();
        std::fs::create_dir_all(grok.join("hooks")).unwrap();
        std::fs::write(root.join("AGENTS.md"), "repo rules").unwrap();
        std::fs::write(grok.join("rules").join("extra.md"), "extra").unwrap();
        std::fs::write(
            grok.join("config.toml"),
            "[mcp_servers.filesystem]\ncommand = \"npx\"\nargs = [\"-y\", \"fs\"]\n",
        )
        .unwrap();
        std::fs::write(
            grok.join("hooks").join("safety.json"),
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"safety-check","timeout":10}]}]}}"#,
        )
        .unwrap();

        let items = scan_provider(ProviderId::Grok, &root, Scope::Project);
        assert!(items
            .iter()
            .any(|i| i.kind == ItemKind::InstructionFile && i.name == "AGENTS.md"));
        assert!(items
            .iter()
            .any(|i| i.kind == ItemKind::Rule && i.name == "extra.md"));
        assert!(items
            .iter()
            .any(|i| i.kind == ItemKind::Skill && i.name == "review"));
        let fs_server = items
            .iter()
            .find(|i| i.kind == ItemKind::Mcp && i.name == "filesystem")
            .unwrap();
        assert_eq!(fs_server.state, ItemState::Enabled);
        assert!(matches!(
            fs_server.toggle_spec,
            Some(ToggleSpec::TomlFlag { .. })
        ));
        let hook = items
            .iter()
            .find(|i| i.kind == ItemKind::Hook && i.name == "safety-check")
            .unwrap();
        assert_eq!(hook.state, ItemState::Enabled);
    }

    #[test]
    fn grok_toml_mcp_toggles_the_enabled_flag_in_place() {
        let root = crate::test_env::temp_dir("scanner-grok-mcp-toggle");
        let grok = root.join(".grok");
        std::fs::create_dir_all(&grok).unwrap();
        std::fs::write(
            grok.join("config.toml"),
            "[mcp_servers.filesystem]\ncommand = \"npx\"\nenabled = true\n",
        )
        .unwrap();

        let items = scan_provider(ProviderId::Grok, &root, Scope::Project);
        let mut server = items
            .iter()
            .find(|i| i.kind == ItemKind::Mcp && i.name == "filesystem")
            .unwrap()
            .clone();
        crate::toggler::toggle_item(&mut server).unwrap();
        assert_eq!(server.state, ItemState::Disabled);
        let text = std::fs::read_to_string(grok.join("config.toml")).unwrap();
        assert!(
            text.contains("enabled = false"),
            "toml must keep its comment-free shape: {text}"
        );
    }

    #[test]
    fn zcode_disabled_entry_flag_is_reported() {
        let root = crate::test_env::temp_dir("scanner-zcode-flag");
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
