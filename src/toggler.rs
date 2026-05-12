use crate::types::*;
use std::path::Path;
use anyhow::Result;

pub fn toggle_item(item: &mut ConfigItem) -> Result<()> {
    if let Some(loc) = item.hook_loc.clone() {
        return toggle_hook(item, &loc);
    }
    match item.state {
        ItemState::Enabled => {
            let dst = item.disabled_path();
            std::fs::rename(&item.path, &dst)?;
            item.path = dst;
            item.state = ItemState::Disabled;
        }
        ItemState::Disabled => {
            let dst = item.enabled_path();
            std::fs::rename(&item.path, &dst)?;
            item.path = dst;
            item.state = ItemState::Enabled;
        }
    }
    Ok(())
}

fn toggle_hook(item: &mut ConfigItem, loc: &HookLoc) -> Result<()> {
    if item.provider == ProviderId::Gemini {
        return toggle_gemini_hook(item, loc);
    }
    toggle_hook_stash(item, loc)
}

fn toggle_gemini_hook(item: &mut ConfigItem, loc: &HookLoc) -> Result<()> {
    backup(&item.path)?;
    let mut doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&item.path)?)?;
    let hooks = doc.get_mut("hooks").and_then(|v| v.as_object_mut())
        .ok_or_else(|| anyhow::anyhow!("no hooks object"))?;
    let disabled = hooks.entry("disabled").or_insert_with(|| serde_json::json!([]));
    let arr = disabled.as_array_mut().ok_or_else(|| anyhow::anyhow!("disabled not array"))?;
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
            if obj.values().all(|v| v.as_array().map_or(true, |a| a.is_empty())) {
                doc.as_object_mut().unwrap().remove("_agentswitch_disabled");
            }
        }
        item.state = ItemState::Enabled;
    }
    std::fs::write(&item.path, serde_json::to_string_pretty(&doc)?)?;
    Ok(())
}

fn remove_from_array(doc: &mut serde_json::Value, section: &str, event: &str, index: usize) -> Result<serde_json::Value> {
    let arr = doc.get_mut(section).and_then(|v| v.get_mut(event)).and_then(|v| v.as_array_mut())
        .ok_or_else(|| anyhow::anyhow!("{}.{} not found", section, event))?;
    if index >= arr.len() { anyhow::bail!("index {} >= len {}", index, arr.len()); }
    Ok(arr.remove(index))
}

fn ensure_array<'a>(doc: &'a mut serde_json::Value, section: &str, event: &str) -> &'a mut Vec<serde_json::Value> {
    let obj = doc.as_object_mut().unwrap();
    let sec = obj.entry(section).or_insert_with(|| serde_json::json!({}));
    let sec_obj = sec.as_object_mut().unwrap();
    sec_obj.entry(event).or_insert_with(|| serde_json::json!([])).as_array_mut().unwrap()
}



fn backup(path: &Path) -> Result<()> {
    std::fs::copy(path, path.with_extension("json.bak"))?;
    Ok(())
}
