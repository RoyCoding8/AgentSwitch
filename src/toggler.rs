use crate::types::*;
use anyhow::Result;
use std::path::Path;

pub fn toggle_item(item: &mut ConfigItem) -> Result<()> {
    if let Some(loc) = item.hook_loc.clone() {
        return toggle_hook(item, &loc);
    }

    if item.kind == ItemKind::Mcp || item.kind == ItemKind::Agent {
        if item.path.extension().and_then(|e| e.to_str()) == Some("toml") {
            return toggle_toml_mcp(item);
        } else {
            return toggle_json_item(item);
        }
    }

    match item.state {
        ItemState::Enabled => {
            let dst = item.disabled_path();
            if let Some(p) = dst.parent() {
                std::fs::create_dir_all(p)?;
            }
            std::fs::rename(&item.path, &dst)?;
            item.path = dst;
            item.state = ItemState::Disabled;
        }
        ItemState::Disabled => {
            let dst = item.enabled_path();
            if let Some(p) = dst.parent() {
                std::fs::create_dir_all(p)?;
            }
            std::fs::rename(&item.path, &dst)?;
            item.path = dst;
            item.state = ItemState::Enabled;
        }
    }
    Ok(())
}

fn toggle_hook(item: &mut ConfigItem, loc: &HookLoc) -> Result<()> {
    if item.provider == ProviderId::Gemini || item.provider == ProviderId::Antigravity {
        return toggle_gemini_hook(item, loc);
    }
    toggle_hook_stash(item, loc)
}

fn toggle_gemini_hook(item: &mut ConfigItem, loc: &HookLoc) -> Result<()> {
    backup(&item.path)?;
    let mut doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&item.path)?)?;
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
    if item.state.is_enabled() {
        if !arr.iter().any(|v| v.as_str() == Some(&loc.hook_name)) {
            arr.push(serde_json::Value::String(loc.hook_name.clone()));
        }
        item.state = ItemState::Disabled;
    } else {
        arr.retain(|v| v.as_str() != Some(&loc.hook_name));
        item.state = ItemState::Enabled;
    }
    std::fs::write(&item.path, serde_json::to_string_pretty(&doc)?)?;
    Ok(())
}

fn toggle_hook_stash(item: &mut ConfigItem, loc: &HookLoc) -> Result<()> {
    backup(&item.path)?;
    let mut doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&item.path)?)?;
    if item.state.is_enabled() {
        let entry = remove_from_array(&mut doc, "hooks", &loc.event, loc.index)?;
        ensure_array(&mut doc, "_agentswitch_disabled", &loc.event).push(entry);
        item.state = ItemState::Disabled;
    } else {
        let real_event = loc.event.strip_prefix("_stashed_").unwrap_or(&loc.event);
        let entry = remove_from_array(&mut doc, "_agentswitch_disabled", real_event, loc.index)?;
        ensure_array(&mut doc, "hooks", real_event).push(entry);
        if let Some(obj) = doc.get("_agentswitch_disabled").and_then(|v| v.as_object()) {
            if obj
                .values()
                .all(|v| v.as_array().is_none_or(|a| a.is_empty()))
            {
                doc.as_object_mut().unwrap().remove("_agentswitch_disabled");
            }
        }
        item.state = ItemState::Enabled;
    }
    std::fs::write(&item.path, serde_json::to_string_pretty(&doc)?)?;
    Ok(())
}

fn remove_from_array(
    doc: &mut serde_json::Value,
    section: &str,
    event: &str,
    index: usize,
) -> Result<serde_json::Value> {
    let arr = doc
        .get_mut(section)
        .and_then(|v| v.get_mut(event))
        .and_then(|v| v.as_array_mut())
        .ok_or_else(|| anyhow::anyhow!("{}.{} not found", section, event))?;
    if index >= arr.len() {
        anyhow::bail!("index {} >= len {}", index, arr.len());
    }
    Ok(arr.remove(index))
}

fn ensure_array<'a>(
    doc: &'a mut serde_json::Value,
    section: &str,
    event: &str,
) -> &'a mut Vec<serde_json::Value> {
    let obj = doc.as_object_mut().unwrap();
    let sec = obj.entry(section).or_insert_with(|| serde_json::json!({}));
    let sec_obj = sec.as_object_mut().unwrap();
    sec_obj
        .entry(event)
        .or_insert_with(|| serde_json::json!([]))
        .as_array_mut()
        .unwrap()
}

fn backup(path: &Path) -> Result<()> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("json");
    std::fs::copy(path, path.with_extension(format!("{ext}.bak")))?;
    Ok(())
}

fn toggle_toml_mcp(item: &mut ConfigItem) -> Result<()> {
    backup(&item.path)?;
    let text = std::fs::read_to_string(&item.path)?;
    let mut doc: toml::Value = toml::from_str(&text)?;
    let servers = doc
        .get_mut("mcp_servers")
        .and_then(|v| v.as_table_mut())
        .ok_or_else(|| anyhow::anyhow!("no mcp_servers table"))?;

    let server = servers
        .get_mut(&item.name)
        .and_then(|v| v.as_table_mut())
        .ok_or_else(|| anyhow::anyhow!("server {} not found", item.name))?;

    if item.state.is_enabled() {
        server.insert("enabled".to_string(), toml::Value::Boolean(false));
        item.state = ItemState::Disabled;
    } else {
        server.insert("enabled".to_string(), toml::Value::Boolean(true));
        item.state = ItemState::Enabled;
    }

    std::fs::write(&item.path, toml::to_string_pretty(&doc)?)?;
    Ok(())
}

fn toggle_json_item(item: &mut ConfigItem) -> Result<()> {
    backup(&item.path)?;
    let mut doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&item.path)?)?;

    let candidates = ["mcpServers", "mcp", "agent"];
    let mut found = false;
    for key in candidates {
        if let Some(obj) = doc.get_mut(key).and_then(|v| v.as_object_mut()) {
            if let Some(val) = obj.get_mut(&item.name) {
                if let Some(o) = val.as_object_mut() {
                    if item.state.is_enabled() {
                        if key == "mcp" || key == "agent" {
                            o.insert("enabled".to_string(), serde_json::Value::Bool(false));
                        } else {
                            o.insert("disabled".to_string(), serde_json::Value::Bool(true));
                        }
                        item.state = ItemState::Disabled;
                    } else {
                        if key == "mcp" || key == "agent" {
                            o.insert("enabled".to_string(), serde_json::Value::Bool(true));
                        } else {
                            o.remove("disabled");
                        }
                        item.state = ItemState::Enabled;
                    }
                    found = true;
                    break;
                }
            }
        }

        let disabled_key = format!("_disabled_{}", key);
        if let Some(obj) = doc.get_mut(&disabled_key).and_then(|v| v.as_object_mut()) {
            if let Some(mut val) = obj.remove(&item.name) {
                if let Some(o) = val.as_object_mut() {
                    if item.state.is_enabled() {
                        if key == "mcp" || key == "agent" {
                            o.insert("enabled".to_string(), serde_json::Value::Bool(false));
                        } else {
                            o.insert("disabled".to_string(), serde_json::Value::Bool(true));
                        }
                        item.state = ItemState::Disabled;
                    } else {
                        if key == "mcp" || key == "agent" {
                            o.insert("enabled".to_string(), serde_json::Value::Bool(true));
                        } else {
                            o.remove("disabled");
                        }
                        item.state = ItemState::Enabled;
                    }
                }
                let main_obj = doc
                    .as_object_mut()
                    .unwrap()
                    .entry(key)
                    .or_insert_with(|| serde_json::json!({}));
                main_obj
                    .as_object_mut()
                    .unwrap()
                    .insert(item.name.clone(), val);

                if doc
                    .get(&disabled_key)
                    .and_then(|v| v.as_object())
                    .is_some_and(|o| o.is_empty())
                {
                    doc.as_object_mut().unwrap().remove(&disabled_key);
                }

                found = true;
                break;
            }
        }
    }
    if !found {
        anyhow::bail!("Item '{}' not found in JSON", item.name);
    }
    std::fs::write(&item.path, serde_json::to_string_pretty(&doc)?)?;
    Ok(())
}
