use crate::config_store::{move_path, Snapshot};
use crate::types::*;
use anyhow::Result;

pub fn toggle_item(item: &mut ConfigItem) -> Result<()> {
    if item.kind == ItemKind::Plugin {
        anyhow::bail!("Plugins cannot be toggled directly; edit opencode.json instead");
    }
    if item.kind == ItemKind::Hook
        && item.provider == ProviderId::Codex
        && item.path.extension() == Some(std::ffi::OsStr::new("toml"))
    {
        anyhow::bail!("Codex TOML hook '{}' is read-only", item.name);
    }

    if let Some(loc) = item.hook_loc.clone() {
        return toggle_hook(item, &loc);
    }

    if let Some(spec) = item.toggle_spec.clone() {
        return toggle_structured_item(item, &spec);
    }

    if item.kind == ItemKind::Mcp || (item.kind == ItemKind::Agent && !item.path.exists()) {
        anyhow::bail!("No safe toggle strategy is available for '{}'", item.name);
    }

    match item.state {
        ItemState::Enabled => {
            let dst = item.disabled_path();
            if let Some(p) = dst.parent() {
                std::fs::create_dir_all(p)?;
            }
            move_path(&item.path, &dst)?;
            item.path = dst;
            item.state = ItemState::Disabled;
        }
        ItemState::Disabled => {
            let dst = item.enabled_path();
            if let Some(p) = dst.parent() {
                std::fs::create_dir_all(p)?;
            }
            move_path(&item.path, &dst)?;
            item.path = dst;
            item.state = ItemState::Enabled;
        }
    }
    Ok(())
}

fn toggle_hook(item: &mut ConfigItem, loc: &HookLoc) -> Result<()> {
    match item.provider {
        ProviderId::Antigravity => return toggle_antigravity_hook(item, loc),
        ProviderId::Zcode => return toggle_zcode_hook(item, loc),
        _ => {}
    }
    toggle_hook_stash(item, loc)
}

fn toggle_zcode_hook(item: &mut ConfigItem, loc: &HookLoc) -> Result<()> {
    let snapshot = Snapshot::read(&item.path)?;
    let mut doc: serde_json::Value = serde_json::from_str(snapshot.text()?)?;
    let arr = array_at_mut(&mut doc, &loc.section, &loc.event)?;
    let enable = !item.state.is_enabled();
    let entry = arr
        .iter_mut()
        .find(|entry| zcode_entry_fingerprint(entry) == loc.fingerprint)
        .ok_or_else(|| anyhow::anyhow!("hook no longer exists in {}.{}", loc.section, loc.event))?;
    let obj = entry
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("hook entry is not an object"))?;
    if enable {
        obj.remove("enabled");
        item.state = ItemState::Enabled;
    } else {
        obj.insert("enabled".into(), serde_json::Value::Bool(false));
        item.state = ItemState::Disabled;
    }
    snapshot.commit(serde_json::to_string_pretty(&doc)?.as_bytes())?;
    Ok(())
}

fn toggle_antigravity_hook(item: &mut ConfigItem, loc: &HookLoc) -> Result<()> {
    let snapshot = Snapshot::read(&item.path)?;
    let mut doc: serde_json::Value = serde_json::from_str(snapshot.text()?)?;
    let hooks = doc
        .get_mut("hooks")
        .and_then(|v| v.as_object_mut())
        .ok_or_else(|| anyhow::anyhow!("no hooks object"))?;
    let disabled = hooks
        .entry("disabled")
        .or_insert_with(|| serde_json::json!([]));
    let arr = disabled
        .as_array_mut()
        .ok_or_else(|| anyhow::anyhow!("disabled not array"))?;
    let scoped_name = format!("{}:{}", loc.event, loc.hook_name);
    if item.state.is_enabled() {
        arr.retain(|v| v.as_str() != Some(loc.hook_name.as_str()));
        if !arr.iter().any(|v| v.as_str() == Some(&scoped_name)) {
            arr.push(serde_json::Value::String(scoped_name));
        }
        item.state = ItemState::Disabled;
    } else {
        arr.retain(|v| {
            v.as_str() != Some(&scoped_name) && v.as_str() != Some(loc.hook_name.as_str())
        });
        item.state = ItemState::Enabled;
    }
    snapshot.commit(serde_json::to_string_pretty(&doc)?.as_bytes())?;
    Ok(())
}

fn toggle_hook_stash(item: &mut ConfigItem, loc: &HookLoc) -> Result<()> {
    let snapshot = Snapshot::read(&item.path)?;
    let mut doc: serde_json::Value = serde_json::from_str(snapshot.text()?)?;
    if item.state.is_enabled() {
        let mut entry = remove_hook(&mut doc, &loc.section, &loc.event, &loc.fingerprint)?;
        if let Some(obj) = entry.as_object_mut() {
            obj.insert("_agentswitch_order".into(), serde_json::json!(loc.order));
        }
        ensure_array(&mut doc, "_agentswitch_disabled", &loc.event)?.push(entry);
        item.state = ItemState::Disabled;
    } else {
        let real_event = loc.event.strip_prefix("_stashed_").unwrap_or(&loc.event);
        let mut entry = remove_hook(
            &mut doc,
            "_agentswitch_disabled",
            real_event,
            &loc.fingerprint,
        )?;
        let original_order = entry
            .get("_agentswitch_order")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(loc.order as u64) as usize;
        let stashed_orders: Vec<usize> = doc
            .get("_agentswitch_disabled")
            .and_then(|stash| stash.get(real_event))
            .and_then(serde_json::Value::as_array)
            .map_or(vec![], |entries| {
                entries
                    .iter()
                    .chain(std::iter::once(&entry))
                    .filter_map(|e| e.get("_agentswitch_order"))
                    .filter_map(serde_json::Value::as_u64)
                    .map(|o| o as usize)
                    .collect()
            });
        if let Some(obj) = entry.as_object_mut() {
            obj.remove("_agentswitch_order");
        }
        let arr = array_at_mut(&mut doc, &loc.section, real_event)?;
        let mut index = arr.len();
        for (current, _) in arr.iter().enumerate() {
            let mut original = current;
            loop {
                let shifted = current + stashed_orders.iter().filter(|&&o| o <= original).count();
                if shifted == original {
                    break;
                }
                original = shifted;
            }
            if original > original_order {
                index = current;
                break;
            }
        }
        arr.insert(index, entry);
        let stash_is_empty = doc
            .get("_agentswitch_disabled")
            .and_then(|v| v.as_object())
            .is_some_and(|obj| {
                obj.values()
                    .all(|v| v.as_array().is_some_and(|a| a.is_empty()))
            });
        if stash_is_empty {
            doc.as_object_mut()
                .ok_or_else(|| anyhow::anyhow!("configuration root is not an object"))?
                .remove("_agentswitch_disabled");
        }
        item.state = ItemState::Enabled;
    }
    snapshot.commit(serde_json::to_string_pretty(&doc)?.as_bytes())?;
    Ok(())
}

fn remove_hook(
    doc: &mut serde_json::Value,
    section: &str,
    event: &str,
    fingerprint: &str,
) -> Result<serde_json::Value> {
    let identity = |entry: &serde_json::Value| {
        let mut stripped = entry.clone();
        if let Some(obj) = stripped.as_object_mut() {
            obj.remove("_agentswitch_order");
        }
        hook_fingerprint(&stripped)
    };
    let arr = array_at_mut(doc, section, event)?;
    let matches: Vec<_> = arr
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| (identity(entry) == fingerprint).then_some(index))
        .collect();
    match matches.as_slice() {
        [index] => Ok(arr.remove(*index)),
        [] => anyhow::bail!("hook no longer exists in {section}.{event}"),
        _ => anyhow::bail!("hook identity is ambiguous in {section}.{event}"),
    }
}

pub(crate) fn zcode_entry_fingerprint(entry: &serde_json::Value) -> String {
    let mut stripped = entry.clone();
    if let Some(object) = stripped.as_object_mut() {
        object.remove("enabled");
    }
    hook_fingerprint(&stripped)
}

fn array_at_mut<'a>(
    doc: &'a mut serde_json::Value,
    section: &str,
    event: &str,
) -> Result<&'a mut Vec<serde_json::Value>> {
    ensure_array(doc, section, event)
}

fn ensure_array<'a>(
    doc: &'a mut serde_json::Value,
    section: &str,
    event: &str,
) -> Result<&'a mut Vec<serde_json::Value>> {
    let segments: Vec<&str> = section.split('/').filter(|s| !s.is_empty()).collect();
    let obj = ensure_object_path(doc, &segments)?;
    obj.entry(event.to_string())
        .or_insert_with(|| serde_json::json!([]))
        .as_array_mut()
        .ok_or_else(|| anyhow::anyhow!("{section}.{event} is not an array"))
}

fn ensure_object_path<'a>(
    doc: &'a mut serde_json::Value,
    segments: &[&str],
) -> Result<&'a mut serde_json::Map<String, serde_json::Value>> {
    let not_object = || anyhow::anyhow!("configuration root is not an object");
    let Some((first, rest)) = segments.split_first() else {
        return doc.as_object_mut().ok_or_else(not_object);
    };
    let child = doc
        .as_object_mut()
        .ok_or_else(not_object)?
        .entry(first.to_string())
        .or_insert_with(|| serde_json::json!({}));
    if rest.is_empty() {
        return child
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("{first} is not an object"));
    }
    ensure_object_path(child, rest)
}

fn hook_fingerprint(value: &serde_json::Value) -> String {
    fn canonical(value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Array(values) => {
                serde_json::Value::Array(values.iter().map(canonical).collect())
            }
            serde_json::Value::Object(object) => {
                let mut keys: Vec<_> = object.keys().collect();
                keys.sort();
                let mut sorted = serde_json::Map::new();
                for key in keys {
                    sorted.insert(key.clone(), canonical(&object[key]));
                }
                serde_json::Value::Object(sorted)
            }
            _ => value.clone(),
        }
    }

    serde_json::to_string(&canonical(value)).unwrap_or_else(|_| value.to_string())
}

fn commit_json(
    item: &mut ConfigItem,
    snapshot: &Snapshot,
    doc: &serde_json::Value,
    enable: bool,
) -> Result<()> {
    snapshot.commit(serde_json::to_string_pretty(doc)?.as_bytes())?;
    item.state = if enable {
        ItemState::Enabled
    } else {
        ItemState::Disabled
    };
    Ok(())
}

fn toggle_structured_item(item: &mut ConfigItem, spec: &ToggleSpec) -> Result<()> {
    match spec {
        ToggleSpec::JsonFlag {
            section,
            name,
            flag,
            enabled_value,
            disabled_value,
        } => toggle_json_flag(item, section, name, flag, *enabled_value, *disabled_value),
        ToggleSpec::TomlFlag {
            section,
            name,
            flag,
            enabled_value,
            disabled_value,
        } => toggle_toml_flag(item, section, name, flag, *enabled_value, *disabled_value),
        ToggleSpec::StringLists {
            path,
            enabled_key,
            disabled_key,
            name,
        } => toggle_string_lists(item, path, enabled_key, disabled_key, name),
        ToggleSpec::JsonStash { section, name } => toggle_json_stash(item, section, name),
    }
}

fn toggle_json_flag(
    item: &mut ConfigItem,
    section: &str,
    name: &str,
    flag: &str,
    enabled_value: bool,
    disabled_value: bool,
) -> Result<()> {
    let snapshot = Snapshot::read(&item.path)?;
    let mut doc: serde_json::Value = serde_json::from_str(snapshot.text()?)?;
    let segments: Vec<&str> = section.split('/').filter(|s| !s.is_empty()).collect();
    let section_obj = ensure_object_path(&mut doc, &segments)?;
    let entry = section_obj
        .get_mut(name)
        .and_then(|value| value.as_object_mut())
        .ok_or_else(|| anyhow::anyhow!("{section}.{name} is not an object"))?;
    let enable = !item.state.is_enabled();
    entry.insert(
        flag.into(),
        serde_json::Value::Bool(if enable {
            enabled_value
        } else {
            disabled_value
        }),
    );
    commit_json(item, &snapshot, &doc, enable)
}

fn toggle_toml_flag(
    item: &mut ConfigItem,
    section: &str,
    name: &str,
    flag: &str,
    enabled_value: bool,
    disabled_value: bool,
) -> Result<()> {
    let snapshot = Snapshot::read(&item.path)?;
    let mut doc: toml_edit::DocumentMut = snapshot
        .text()?
        .parse()
        .map_err(|error| anyhow::anyhow!("invalid TOML in {}: {error}", item.path.display()))?;
    fn inline_to_regular(item: &mut toml_edit::Item) -> Option<&mut toml_edit::Table> {
        if item.is_inline_table() {
            let inline = item.as_inline_table_mut()?;
            let mut table = toml_edit::Table::new();
            table.set_implicit(true);
            for (key, value) in inline.iter() {
                table.insert(key, toml_edit::Item::Value(value.clone()));
            }
            *item = toml_edit::Item::Table(table);
        }
        item.as_table_mut()
    }
    let nested = doc
        .get_mut(section)
        .and_then(|value| value.as_table_mut())
        .and_then(|table| table.get_mut(name))
        .and_then(inline_to_regular);
    let table = match nested {
        Some(table) => table,
        None => doc
            .get_mut(&format!("{section}.{name}"))
            .and_then(inline_to_regular)
            .ok_or_else(|| anyhow::anyhow!("{section}.{name} is not a table"))?,
    };
    let enable = !item.state.is_enabled();
    table.insert(
        flag,
        toml_edit::value(if enable {
            enabled_value
        } else {
            disabled_value
        }),
    );
    snapshot.commit(doc.to_string().as_bytes())?;
    item.state = if enable {
        ItemState::Enabled
    } else {
        ItemState::Disabled
    };
    Ok(())
}

fn toggle_string_lists(
    item: &mut ConfigItem,
    path: &std::path::Path,
    enabled_key: &str,
    disabled_key: &str,
    name: &str,
) -> Result<()> {
    let snapshot = Snapshot::read_or(path, b"{}")?;
    let mut doc: serde_json::Value = serde_json::from_str(snapshot.text()?)?;
    let enable = !item.state.is_enabled();
    remove_string(&mut doc, enabled_key, name)?;
    remove_string(&mut doc, disabled_key, name)?;
    let target_key = if enable { enabled_key } else { disabled_key };
    ensure_string_array(&mut doc, target_key)?.push(name.into());
    commit_json(item, &snapshot, &doc, enable)
}

fn toggle_json_stash(item: &mut ConfigItem, section: &str, name: &str) -> Result<()> {
    let snapshot = Snapshot::read(&item.path)?;
    let mut doc: serde_json::Value = serde_json::from_str(snapshot.text()?)?;
    let enable = !item.state.is_enabled();
    let disabled_section = format!("_disabled_{}", section.replace('/', "_"));
    let (source_obj, target_key) = if enable {
        (disabled_section.as_str(), section)
    } else {
        (section, disabled_section.as_str())
    };
    let source_segments: Vec<&str> = source_obj.split('/').filter(|s| !s.is_empty()).collect();
    let value = ensure_object_path(&mut doc, &source_segments)
        .ok()
        .and_then(|object| object.remove(name))
        .ok_or_else(|| anyhow::anyhow!("{source_obj}.{name} not found"))?;
    let target_segments: Vec<&str> = target_key.split('/').filter(|s| !s.is_empty()).collect();
    ensure_object_path(&mut doc, &target_segments)?.insert(name.into(), value);
    if enable {
        if let Some(obj) = doc.as_object_mut() {
            let empty = obj
                .get(disabled_section.as_str())
                .and_then(|v| v.as_object())
                .is_some_and(|stash| stash.is_empty());
            if empty {
                obj.remove(disabled_section.as_str());
            }
        }
    }
    commit_json(item, &snapshot, &doc, enable)
}

fn ensure_string_array<'a>(
    doc: &'a mut serde_json::Value,
    key: &str,
) -> Result<&'a mut Vec<serde_json::Value>> {
    doc.as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("configuration root is not an object"))?
        .entry(key)
        .or_insert_with(|| serde_json::json!([]))
        .as_array_mut()
        .ok_or_else(|| anyhow::anyhow!("{key} is not an array"))
}

fn remove_string(doc: &mut serde_json::Value, key: &str, name: &str) -> Result<()> {
    if let Some(value) = doc.get_mut(key) {
        let array = value
            .as_array_mut()
            .ok_or_else(|| anyhow::anyhow!("{key} is not an array"))?;
        array.retain(|value| value.as_str() != Some(name));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_file(name: &str, content: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("agentswitch-toggler-{name}-{nonce}"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, content).unwrap();
        path
    }

    fn hook_item(path: std::path::PathBuf, entry: &serde_json::Value, name: &str) -> ConfigItem {
        let mut item = ConfigItem::new(name, ItemKind::Hook, path, ProviderId::Claude);
        item.hook_loc = Some(HookLoc {
            section: "hooks".into(),
            event: "PreToolUse".into(),
            order: 0,
            hook_name: name.into(),
            fingerprint: hook_fingerprint(entry),
        });
        item
    }

    #[test]
    fn hook_toggle_uses_content_identity_after_sibling_moves() {
        let first =
            serde_json::json!({"matcher":"Bash","hooks":[{"type":"command","command":"first"}]});
        let second =
            serde_json::json!({"matcher":"Edit","hooks":[{"type":"command","command":"second"}]});
        let path = temp_file(
            "identity",
            &serde_json::json!({"hooks":{"PreToolUse":[first.clone(), second.clone()]}})
                .to_string(),
        );
        let mut first_item = hook_item(path.clone(), &first, "first");
        let mut second_item = hook_item(path.clone(), &second, "second");

        toggle_item(&mut first_item).unwrap();
        toggle_item(&mut second_item).unwrap();

        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(doc["hooks"]["PreToolUse"].as_array().unwrap().len(), 0);
        let stashed = doc["_agentswitch_disabled"]["PreToolUse"]
            .as_array()
            .unwrap();
        let stripped: Vec<_> = stashed
            .iter()
            .map(|entry| {
                let mut entry = entry.clone();
                if let Some(obj) = entry.as_object_mut() {
                    obj.remove("_agentswitch_order");
                }
                entry
            })
            .collect();
        assert_eq!(stripped, &[first, second]);
    }

    #[test]
    fn malformed_stash_returns_error_instead_of_panicking() {
        let entry = serde_json::json!({"hooks":[{"command":"first"}]});
        let path = temp_file(
            "malformed",
            &serde_json::json!({
                "hooks":{"PreToolUse":[entry.clone()]},
                "_agentswitch_disabled":"broken"
            })
            .to_string(),
        );
        let mut item = hook_item(path, &entry, "first");
        let error = toggle_item(&mut item).unwrap_err().to_string();
        assert!(error.contains("_agentswitch_disabled is not an object"));
    }

    #[test]
    fn filesystem_agent_toggle_renames_and_restores_directory() {
        let path = temp_file("agent-dir", "agent");
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("config.json"), "{}").unwrap();
        let mut item = ConfigItem::new("agent", ItemKind::Agent, path.clone(), ProviderId::Kiro);

        toggle_item(&mut item).unwrap();
        assert_eq!(item.state, ItemState::Disabled);
        assert!(!path.exists());
        assert_eq!(
            std::fs::read_to_string(item.path.join("config.json")).unwrap(),
            "{}"
        );

        toggle_item(&mut item).unwrap();
        assert_eq!(item.state, ItemState::Enabled);
        assert_eq!(item.path, path);
        assert_eq!(
            std::fs::read_to_string(path.join("config.json")).unwrap(),
            "{}"
        );
    }

    #[test]
    fn codex_toml_hook_is_reported_as_read_only() {
        let mut path = temp_file("codex-hook", "[hooks]\n");
        path.set_extension("toml");
        std::fs::write(&path, "[hooks]\n").unwrap();
        let mut item = ConfigItem::new("notify", ItemKind::Hook, path, ProviderId::Codex);
        item.hook_loc = Some(HookLoc {
            section: "hooks".into(),
            event: "notify".into(),
            order: 0,
            hook_name: "notify".into(),
            fingerprint: "fingerprint".into(),
        });

        let error = toggle_item(&mut item).unwrap_err().to_string();
        assert!(error.contains("Codex TOML hook 'notify' is read-only"));
    }

    #[test]
    fn claude_project_mcp_toggle_updates_approval_lists() {
        let mcp_path = temp_file(
            "claude-mcp",
            r#"{"mcpServers":{"docs":{"type":"http","url":"https://example.test"}}}"#,
        );
        let settings_path = mcp_path.parent().unwrap().join("settings.local.json");
        std::fs::write(&settings_path, "{}").unwrap();
        let mut item = ConfigItem::new("docs", ItemKind::Mcp, mcp_path, ProviderId::Claude);
        item.toggle_spec = Some(ToggleSpec::StringLists {
            path: settings_path.clone(),
            enabled_key: "enabledMcpjsonServers".into(),
            disabled_key: "disabledMcpjsonServers".into(),
            name: "docs".into(),
        });

        toggle_item(&mut item).unwrap();
        let settings: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(settings_path).unwrap()).unwrap();
        assert_eq!(
            settings["disabledMcpjsonServers"],
            serde_json::json!(["docs"])
        );
        assert!(settings.get("enabledMcpjsonServers").is_none());
    }

    #[test]
    fn antigravity_mcp_toggle_uses_disabled_flag() {
        let path = temp_file(
            "antigravity-mcp",
            r#"{"mcpServers":{"docs":{"command":"server"}}}"#,
        );
        let mut item =
            ConfigItem::new("docs", ItemKind::Mcp, path.clone(), ProviderId::Antigravity);
        item.toggle_spec = Some(ToggleSpec::JsonFlag {
            section: "mcpServers".into(),
            name: "docs".into(),
            flag: "disabled".into(),
            enabled_value: false,
            disabled_value: true,
        });

        toggle_item(&mut item).unwrap();
        let config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(config["mcpServers"]["docs"]["disabled"], true);
    }

    #[test]
    fn zcode_hook_toggle_uses_native_enabled_flag() {
        let first =
            serde_json::json!({"matcher":"Bash","hooks":[{"type":"process","command":"check"}]});
        let path = temp_file(
            "zcode-hook",
            &serde_json::json!({"hooks":{"enabled":true,"events":{"PreToolUse":[first.clone()]}}})
                .to_string(),
        );
        let mut item = ConfigItem::new("check", ItemKind::Hook, path.clone(), ProviderId::Zcode);
        item.hook_loc = Some(HookLoc {
            section: "hooks/events".into(),
            event: "PreToolUse".into(),
            order: 0,
            hook_name: "check".into(),
            fingerprint: hook_fingerprint(&first),
        });

        toggle_item(&mut item).unwrap();
        assert_eq!(item.state, ItemState::Disabled);
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let entry = &doc["hooks"]["events"]["PreToolUse"][0];
        assert_eq!(entry["enabled"], serde_json::json!(false));
        assert_eq!(entry["matcher"], "Bash", "entry must stay in place");

        toggle_item(&mut item).unwrap();
        assert_eq!(item.state, ItemState::Enabled);
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(doc["hooks"]["events"]["PreToolUse"][0]
            .get("enabled")
            .is_none());
    }

    #[test]
    fn zcode_mcp_stash_moves_servers_out_of_mcp_servers() {
        let path = temp_file(
            "zcode-mcp",
            r#"{"mcp":{"servers":{"docs":{"type":"stdio","command":"ctx"}}}}"#,
        );
        let mut item = ConfigItem::new("docs", ItemKind::Mcp, path.clone(), ProviderId::Zcode);
        item.toggle_spec = Some(ToggleSpec::JsonStash {
            section: "mcp/servers".into(),
            name: "docs".into(),
        });

        toggle_item(&mut item).unwrap();
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(doc["mcp"]["servers"].get("docs").is_none());
        assert_eq!(
            doc["_disabled_mcp_servers"]["docs"]["command"], "ctx",
            "disabled server keeps its definition"
        );

        toggle_item(&mut item).unwrap();
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(doc["mcp"]["servers"]["docs"]["command"], "ctx");
        assert!(doc.get("_disabled_mcp_servers").is_none());
    }

    #[test]
    fn reenabling_a_middle_hook_restores_its_original_order() {
        let a = serde_json::json!({"matcher":"A","hooks":[{"type":"command","command":"a"}]});
        let b = serde_json::json!({"matcher":"B","hooks":[{"type":"command","command":"b"}]});
        let c = serde_json::json!({"matcher":"C","hooks":[{"type":"command","command":"c"}]});
        let path = temp_file(
            "order",
            &serde_json::json!({"hooks":{"PostToolUse":[a.clone(), b.clone(), c.clone()]}})
                .to_string(),
        );
        let make = |entry: &serde_json::Value, order: usize| {
            let mut item =
                ConfigItem::new("hook", ItemKind::Hook, path.clone(), ProviderId::Claude);
            item.hook_loc = Some(HookLoc {
                section: "hooks".into(),
                event: "PostToolUse".into(),
                order,
                hook_name: "hook".into(),
                fingerprint: hook_fingerprint(entry),
            });
            item
        };
        let mut middle = make(&b, 1);
        toggle_item(&mut middle).unwrap();

        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(doc["hooks"]["PostToolUse"].as_array().unwrap().len(), 2);

        toggle_item(&mut middle).unwrap();
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let restored = doc["hooks"]["PostToolUse"].as_array().unwrap();
        assert_eq!(restored.len(), 3);
        assert_eq!(
            restored[1]["matcher"], "B",
            "middle hook goes back to index 1"
        );
    }

    #[test]
    fn reenabling_after_a_earlier_disable_keeps_relative_order() {
        let a = serde_json::json!({"matcher":"A","hooks":[{"type":"command","command":"a"}]});
        let b = serde_json::json!({"matcher":"B","hooks":[{"type":"command","command":"b"}]});
        let c = serde_json::json!({"matcher":"C","hooks":[{"type":"command","command":"c"}]});
        let d = serde_json::json!({"matcher":"D","hooks":[{"type":"command","command":"d"}]});
        let path = temp_file(
            "order-shift",
            &serde_json::json!({"hooks":{"PostToolUse":[a.clone(), b.clone(), c.clone(), d.clone()]}})
                .to_string(),
        );
        let make = |entry: &serde_json::Value, order: usize| {
            let mut item =
                ConfigItem::new("hook", ItemKind::Hook, path.clone(), ProviderId::Claude);
            item.hook_loc = Some(HookLoc {
                section: "hooks".into(),
                event: "PostToolUse".into(),
                order,
                hook_name: "hook".into(),
                fingerprint: hook_fingerprint(entry),
            });
            item
        };
        let mut first = make(&a, 0);
        toggle_item(&mut first).unwrap();
        let mut third = make(&c, 2);
        toggle_item(&mut third).unwrap();

        toggle_item(&mut third).unwrap();
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let restored = doc["hooks"]["PostToolUse"].as_array().unwrap();
        let matchers: Vec<_> = restored
            .iter()
            .map(|entry| entry["matcher"].as_str().unwrap())
            .collect();
        assert_eq!(matchers, ["B", "C", "D"]);

        toggle_item(&mut first).unwrap();
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let restored = doc["hooks"]["PostToolUse"].as_array().unwrap();
        let matchers: Vec<_> = restored
            .iter()
            .map(|entry| entry["matcher"].as_str().unwrap())
            .collect();
        assert_eq!(matchers, ["A", "B", "C", "D"]);
    }

    #[test]
    fn toml_inline_table_toggles_into_a_regular_table() {
        let dir = std::env::temp_dir().join(format!(
            "agentswitch-toml-inline-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            "model = \"gpt-5\"\nmcp_servers.docs = { command = \"docs\" }\n",
        )
        .unwrap();
        let mut item = ConfigItem::new("docs", ItemKind::Mcp, path.clone(), ProviderId::Codex);
        item.toggle_spec = Some(ToggleSpec::TomlFlag {
            section: "mcp_servers".into(),
            name: "docs".into(),
            flag: "enabled".into(),
            enabled_value: true,
            disabled_value: false,
        });

        toggle_item(&mut item).unwrap();
        assert_eq!(item.state, ItemState::Disabled);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("enabled = false"),
            "flag written into converted table: {text}"
        );
        assert!(text.contains("command"), "existing keys survive: {text}");
    }

    #[test]
    fn toml_flag_toggle_preserves_comments_and_layout() {
        let dir = std::env::temp_dir().join(format!(
            "agentswitch-toml-edit-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            "# my precious comment\nmodel = \"gpt-5\"\n\n[mcp_servers.docs]\ncommand = \"docs\"\n",
        )
        .unwrap();
        let mut item = ConfigItem::new("docs", ItemKind::Mcp, path.clone(), ProviderId::Codex);
        item.toggle_spec = Some(ToggleSpec::TomlFlag {
            section: "mcp_servers".into(),
            name: "docs".into(),
            flag: "enabled".into(),
            enabled_value: true,
            disabled_value: false,
        });

        toggle_item(&mut item).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# my precious comment"), "comments survive");
        assert!(text.contains("model = \"gpt-5\""), "layout survives");
        assert!(text.contains("enabled = false"));
    }
}
