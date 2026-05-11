use crate::types::*;
use std::path::Path;
use anyhow::Result;

pub fn toggle_item(item: &mut ConfigItem) -> Result<()> {
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

pub fn toggle_json_key(path: &Path, key: &str, enable: bool) -> Result<()> {
    let bak = path.with_extension("json.bak");
    std::fs::copy(path, &bak)?;
    let text = std::fs::read_to_string(path)?;
    let mut doc: serde_json::Value = serde_json::from_str(&text)?;
    let disabled_key = format!("_disabled_{}", key);
    if enable {
        if let Some(val) = doc.get(&disabled_key).cloned() {
            doc.as_object_mut().unwrap().insert(key.into(), val);
            doc.as_object_mut().unwrap().remove(&disabled_key);
        }
    } else if let Some(val) = doc.get(key).cloned() {
        doc.as_object_mut().unwrap().insert(disabled_key, val);
        doc.as_object_mut().unwrap().remove(key);
    }
    std::fs::write(path, serde_json::to_string_pretty(&doc)?)?;
    Ok(())
}

pub fn set_json_disabled_flag(path: &Path, server_name: &str, disabled: bool) -> Result<()> {
    let bak = path.with_extension("json.bak");
    std::fs::copy(path, &bak)?;
    let text = std::fs::read_to_string(path)?;
    let mut doc: serde_json::Value = serde_json::from_str(&text)?;
    if let Some(servers) = doc.get_mut("mcpServers").and_then(|v| v.as_object_mut()) {
        if let Some(srv) = servers.get_mut(server_name).and_then(|v| v.as_object_mut()) {
            srv.insert("disabled".into(), serde_json::Value::Bool(disabled));
        }
    }
    std::fs::write(path, serde_json::to_string_pretty(&doc)?)?;
    Ok(())
}
