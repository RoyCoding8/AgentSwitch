use crate::config_store::{atomic_write, move_path, Snapshot};
use crate::types::*;
use anyhow::{Context, Result};

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

pub fn sidecar_path(path: &std::path::Path) -> std::path::PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    path.with_file_name(format!("{name}.agentswitch"))
}

fn toggle_hook(item: &mut ConfigItem, loc: &HookLoc) -> Result<()> {
    match item.provider {
        ProviderId::Antigravity if loc.section.is_empty() => {
            return toggle_antigravity_hook(item, loc)
        }
        ProviderId::Kiro
            if item
                .path
                .parent()
                .and_then(|p| p.file_name())
                .is_some_and(|n| n == "hooks") =>
        {
            return toggle_kiro_hook_file(item, loc)
        }
        ProviderId::Zcode if !loc.event.starts_with("_stashed_") => {
            return toggle_zcode_hook(item, loc)
        }
        _ => {}
    }
    toggle_hook_stash(item, loc)
}

fn toggle_zcode_hook(item: &mut ConfigItem, loc: &HookLoc) -> Result<()> {
    let snapshot = Snapshot::read(&item.path)?;
    let mut doc: serde_json::Value = serde_json::from_str(snapshot.text()?)?;
    let arr = array_at_mut(&mut doc, &loc.section, &loc.event)?;
    let fingerprint_matches =
        |entry: &serde_json::Value| zcode_entry_fingerprint(entry) == loc.fingerprint;
    let selected = arr
        .get(loc.order)
        .is_some_and(&fingerprint_matches)
        .then_some(loc.order)
        .or_else(|| {
            let matches: Vec<usize> = arr
                .iter()
                .enumerate()
                .filter(|(_, entry)| fingerprint_matches(entry))
                .map(|(index, _)| index)
                .collect();
            match matches.as_slice() {
                [index] => Some(*index),
                _ => None,
            }
        });
    let entry = selected
        .and_then(|index| arr.get_mut(index))
        .ok_or_else(|| anyhow::anyhow!("hook no longer exists in {}.{}", loc.section, loc.event))?;
    let obj = entry
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("hook entry is not an object"))?;
    let enable = !item.state.is_enabled();
    if enable {
        obj.remove("enabled");
    } else {
        obj.insert("enabled".into(), serde_json::Value::Bool(false));
    }
    snapshot.commit(serde_json::to_string_pretty(&doc)?.as_bytes())?;
    item.state = if enable {
        ItemState::Enabled
    } else {
        ItemState::Disabled
    };
    Ok(())
}

fn toggle_antigravity_hook(item: &mut ConfigItem, loc: &HookLoc) -> Result<()> {
    let snapshot = Snapshot::read(&item.path)?;
    let mut doc: serde_json::Value = serde_json::from_str(snapshot.text()?)?;
    let def = doc
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("configuration root is not an object"))?
        .get_mut(&loc.hook_name)
        .and_then(|v| v.as_object_mut())
        .ok_or_else(|| anyhow::anyhow!("hook '{}' no longer exists", loc.hook_name))?;
    let enable = !item.state.is_enabled();
    if enable {
        def.remove("enabled");
    } else {
        def.insert("enabled".into(), serde_json::Value::Bool(false));
    }
    snapshot.commit(serde_json::to_string_pretty(&doc)?.as_bytes())?;
    item.state = if enable {
        ItemState::Enabled
    } else {
        ItemState::Disabled
    };
    Ok(())
}

fn toggle_kiro_hook_file(item: &mut ConfigItem, loc: &HookLoc) -> Result<()> {
    let snapshot = Snapshot::read(&item.path)?;
    let mut doc: serde_json::Value = serde_json::from_str(snapshot.text()?)?;
    let arr = doc
        .get_mut("hooks")
        .and_then(|v| v.as_array_mut())
        .ok_or_else(|| anyhow::anyhow!("hooks is not an array"))?;
    let enable = !item.state.is_enabled();
    let matches: Vec<usize> = arr
        .iter()
        .enumerate()
        .filter_map(|(i, e)| (kiro_entry_fingerprint(e) == loc.fingerprint).then_some(i))
        .collect();
    let index = match matches.as_slice() {
        [i] => *i,
        [] => anyhow::bail!("hook no longer exists in {}", item.path.display()),
        _ => *matches.iter().find(|&&i| i == loc.order).ok_or_else(|| {
            anyhow::anyhow!("hook identity is ambiguous in {}", item.path.display())
        })?,
    };
    let obj = arr[index]
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("hook entry is not an object"))?;
    if enable {
        obj.remove("enabled");
    } else {
        obj.insert("enabled".into(), serde_json::Value::Bool(false));
    }
    snapshot.commit(serde_json::to_string_pretty(&doc)?.as_bytes())?;
    item.state = if enable {
        ItemState::Enabled
    } else {
        ItemState::Disabled
    };
    Ok(())
}

fn toggle_hook_stash(item: &mut ConfigItem, loc: &HookLoc) -> Result<()> {
    if item.state.is_enabled() {
        stash_hook(item, loc)
    } else {
        unstash_hook(item, loc)
    }
}

fn stash_hook(item: &mut ConfigItem, loc: &HookLoc) -> Result<()> {
    let snapshot = Snapshot::read(&item.path)?;
    let mut doc: serde_json::Value = serde_json::from_str(snapshot.text()?)?;
    let (current_order, mut entry) = remove_hook(
        &mut doc,
        &loc.section,
        &loc.event,
        &loc.fingerprint,
        Some(loc.order),
    )?;
    let sidecar = Snapshot::read_or(&sidecar_path(&item.path), b"{}")?;
    let mut stash: serde_json::Value = serde_json::from_str(sidecar.text()?)?;
    let mut disabled_orders: Vec<_> = stash
        .get(&loc.event)
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("_agentswitch_order"))
        .filter_map(serde_json::Value::as_u64)
        .collect();
    disabled_orders.sort_unstable();
    let mut original_order = current_order as u64;
    for order in disabled_orders {
        if order <= original_order {
            original_order += 1;
        }
    }
    if let Some(obj) = entry.as_object_mut() {
        obj.insert(
            "_agentswitch_order".into(),
            serde_json::json!(original_order),
        );
    }
    ensure_array(&mut stash, "", &loc.event)?.push(entry);
    sidecar.commit(serde_json::to_string_pretty(&stash)?.as_bytes())?;
    if let Err(error) = snapshot.commit(serde_json::to_string_pretty(&doc)?.as_bytes()) {
        let original = sidecar.text()?.to_string();
        let restored = atomic_write(&sidecar_path(&item.path), original.as_bytes());
        let cleaned = match restored {
            Ok(()) if original == "{}" => {
                std::fs::remove_file(sidecar_path(&item.path)).map_err(anyhow::Error::from)
            }
            Ok(()) => Ok(()),
            Err(error) => Err(error),
        };
        if let Err(back_error) = cleaned {
            return Err(error).context(format!(
                "restoring the hook sidecar also failed: {back_error}"
            ));
        }
        return Err(error);
    }
    item.state = ItemState::Disabled;
    Ok(())
}

fn unstash_hook(item: &mut ConfigItem, loc: &HookLoc) -> Result<()> {
    let real_event = loc.event.strip_prefix("_stashed_").unwrap_or(&loc.event);
    let stash_path = sidecar_path(&item.path);
    let sidecar = Snapshot::read_or(&stash_path, b"{}")?;
    let mut stash: serde_json::Value = serde_json::from_str(sidecar.text()?)?;
    if let Ok((_, mut entry)) = remove_hook(
        &mut stash,
        "",
        real_event,
        &loc.fingerprint,
        Some(loc.order),
    ) {
        let original_order = entry
            .get("_agentswitch_order")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(loc.order as u64);
        if let Some(obj) = entry.as_object_mut() {
            obj.remove("_agentswitch_order");
        }
        let other_orders: Vec<u64> = stash
            .get(real_event)
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.get("_agentswitch_order"))
            .filter_map(serde_json::Value::as_u64)
            .chain(std::iter::once(original_order))
            .collect();
        let snapshot = Snapshot::read(&item.path)?;
        let mut doc: serde_json::Value = serde_json::from_str(snapshot.text()?)?;
        let arr = array_at_mut(&mut doc, &loc.section, real_event)?;
        let index = restore_index(arr, original_order as usize, &other_orders);
        arr.insert(index, entry);
        drop_empty_stash(&mut stash);
        snapshot.commit(serde_json::to_string_pretty(&doc)?.as_bytes())?;
        if let Err(error) = sidecar.commit(serde_json::to_string_pretty(&stash)?.as_bytes()) {
            if let Err(back_error) = atomic_write(&item.path, snapshot.text()?.as_bytes()) {
                return Err(error).context(format!(
                    "reverting the hook config also failed: {back_error}"
                ));
            }
            return Err(error);
        }
        if stash.as_object().is_some_and(|o| o.is_empty()) {
            let _ = std::fs::remove_file(&stash_path);
        }
        item.state = ItemState::Enabled;
        return Ok(());
    }
    restore_from_legacy_stash(item, loc, real_event)
}

fn restore_from_legacy_stash(item: &mut ConfigItem, loc: &HookLoc, real_event: &str) -> Result<()> {
    let snapshot = Snapshot::read(&item.path)?;
    let mut doc: serde_json::Value = serde_json::from_str(snapshot.text()?)?;
    let (_, mut entry) = remove_hook(
        &mut doc,
        "_agentswitch_disabled",
        real_event,
        &loc.fingerprint,
        None,
    )?;
    let original_order = entry
        .get("_agentswitch_order")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(loc.order as u64) as usize;
    let other_orders: Vec<u64> = doc
        .get("_agentswitch_disabled")
        .and_then(|stash| stash.get(real_event))
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("_agentswitch_order"))
        .filter_map(serde_json::Value::as_u64)
        .chain(std::iter::once(original_order as u64))
        .collect();
    if let Some(obj) = entry.as_object_mut() {
        obj.remove("_agentswitch_order");
    }
    let arr = array_at_mut(&mut doc, &loc.section, real_event)?;
    let index = restore_index(arr, original_order, &other_orders);
    arr.insert(index, entry);
    if let Some(obj) = doc.as_object_mut() {
        let empty = obj
            .get("_agentswitch_disabled")
            .and_then(|v| v.as_object())
            .is_some_and(|stash| {
                stash
                    .values()
                    .all(|v| v.as_array().is_some_and(|a| a.is_empty()))
            });
        if empty {
            obj.remove("_agentswitch_disabled");
        }
    }
    snapshot.commit(serde_json::to_string_pretty(&doc)?.as_bytes())?;
    item.state = ItemState::Enabled;
    Ok(())
}

fn restore_index(arr: &[serde_json::Value], original_order: usize, other_orders: &[u64]) -> usize {
    let mut index = arr.len();
    for (current, _) in arr.iter().enumerate() {
        let mut original = current;
        loop {
            let shifted = current
                + other_orders
                    .iter()
                    .filter(|&&o| o <= original as u64)
                    .count();
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
    index
}

fn drop_empty_stash(stash: &mut serde_json::Value) {
    if let Some(obj) = stash.as_object_mut() {
        obj.retain(|_, v| v.as_array().is_some_and(|a| !a.is_empty()));
    }
}

fn remove_hook(
    doc: &mut serde_json::Value,
    section: &str,
    event: &str,
    fingerprint: &str,
    prefer_order: Option<usize>,
) -> Result<(usize, serde_json::Value)> {
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
    let index = match matches.as_slice() {
        [index] => *index,
        [] => anyhow::bail!("hook no longer exists in {section}.{event}"),
        _ => {
            if let Some(order) = prefer_order {
                if matches.contains(&order) {
                    order
                } else {
                    anyhow::bail!("hook identity is ambiguous in {section}.{event}")
                }
            } else {
                anyhow::bail!("hook identity is ambiguous in {section}.{event}")
            }
        }
    };
    let removed = arr.remove(index);
    if arr.is_empty() {
        if section.is_empty() {
            if let Some(root) = doc.as_object_mut() {
                root.remove(event);
            }
        } else {
            let section_now_empty = doc
                .get_mut(section)
                .and_then(|value| value.as_object_mut())
                .is_some_and(|object| {
                    object.remove(event);
                    object.is_empty()
                });
            if section_now_empty {
                if let Some(root) = doc.as_object_mut() {
                    root.remove(section);
                }
            }
        }
    }
    Ok((index, removed))
}

pub(crate) fn stash_entry_fingerprint(entry: &serde_json::Value) -> String {
    let mut stripped = entry.clone();
    if let Some(object) = stripped.as_object_mut() {
        object.remove("_agentswitch_order");
    }
    hook_fingerprint(&stripped)
}

pub(crate) fn kiro_entry_fingerprint(entry: &serde_json::Value) -> String {
    let mut stripped = entry.clone();
    if let Some(object) = stripped.as_object_mut() {
        object.remove("enabled");
    }
    hook_fingerprint(&stripped)
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
    let mut doc = crate::config_store::parse_json(&item.path, snapshot.text()?)?;
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
    if item.path.extension().is_some_and(|ext| ext == "jsonc") {
        use jsonc_parser::cst::{CstInputValue, CstObject, CstRootNode};

        let root = CstRootNode::parse(snapshot.text()?, &crate::config_store::jsonc_options())?;
        let unique_property = |object: &CstObject, key: &str| -> Result<_> {
            let matches: Vec<_> = object
                .properties()
                .into_iter()
                .filter(|property| {
                    property
                        .name()
                        .and_then(|name| name.decoded_value().ok())
                        .as_deref()
                        == Some(key)
                })
                .collect();
            if matches.len() > 1 {
                anyhow::bail!("duplicate JSONC property {key}; resolve it before toggling");
            }
            Ok(matches.into_iter().next())
        };
        let mut object = root
            .object_value()
            .ok_or_else(|| anyhow::anyhow!("configuration root is not an object"))?;
        for key in segments.iter().copied().chain(std::iter::once(name)) {
            object = unique_property(&object, key)?
                .and_then(|property| property.object_value())
                .ok_or_else(|| anyhow::anyhow!("{key} is not an object"))?;
        }
        let value = CstInputValue::Bool(if enable {
            enabled_value
        } else {
            disabled_value
        });
        if let Some(property) = unique_property(&object, flag)? {
            property.set_value(value);
        } else {
            object.append(flag, value);
        }
        let text = root.to_string();
        if crate::config_store::parse_json(&item.path, &text)? != doc {
            anyhow::bail!("JSONC edit changed unexpected configuration values");
        }
        snapshot.commit(text.as_bytes())?;
        item.state = if enable {
            ItemState::Enabled
        } else {
            ItemState::Disabled
        };
        Ok(())
    } else {
        commit_json(item, &snapshot, &doc, enable)
    }
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
        .and_then(inline_to_regular)
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
    let target_segments: Vec<&str> = target_key.split('/').filter(|s| !s.is_empty()).collect();
    if ensure_object_path(&mut doc, &target_segments)?.contains_key(name) {
        anyhow::bail!(
            "{target_key}.{name} already has a conflicting definition; refusing to overwrite it"
        );
    }
    let value = ensure_object_path(&mut doc, &source_segments)
        .ok()
        .and_then(|object| object.remove(name))
        .ok_or_else(|| anyhow::anyhow!("{source_obj}.{name} not found"))?;
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

    fn read_doc(path: impl AsRef<std::path::Path>) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }
    fn post_tool_use_item(
        path: std::path::PathBuf,
        entry: &serde_json::Value,
        order: usize,
    ) -> ConfigItem {
        let mut item = ConfigItem::new("hook", ItemKind::Hook, path, ProviderId::Claude);
        item.hook_loc = Some(HookLoc {
            section: "hooks".into(),
            event: "PostToolUse".into(),
            order,
            hook_name: "hook".into(),
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
        let path = crate::test_env::temp_file(
            "toggler-identity",
            "settings.json",
            serde_json::json!({"hooks":{"PreToolUse":[first.clone(), second.clone()]}})
                .to_string()
                .as_bytes(),
        );
        let mut first_item = hook_item(path.clone(), &first, "first");
        let mut second_item = hook_item(path.clone(), &second, "second");

        toggle_item(&mut first_item).unwrap();
        toggle_item(&mut second_item).unwrap();

        let doc = read_doc(&path);
        assert!(
            doc.pointer("/hooks/PreToolUse").is_none(),
            "emptied hook event arrays are dropped, not left behind"
        );
        let stash = read_doc(sidecar_path(&path));
        let stashed = stash["PreToolUse"].as_array().unwrap();
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
    fn malformed_sidecar_fails_the_disable_without_touching_the_config() {
        let entry = serde_json::json!({"hooks":[{"command":"first"}]});
        let path = crate::test_env::temp_file(
            "toggler-malformed",
            "settings.json",
            serde_json::json!({"hooks":{"PreToolUse":[entry.clone()]}})
                .to_string()
                .as_bytes(),
        );
        std::fs::write(sidecar_path(&path), "broken").unwrap();
        let mut item = hook_item(path.clone(), &entry, "first");
        assert!(toggle_item(&mut item).is_err());
        let doc = read_doc(&path);
        assert_eq!(
            doc["hooks"]["PreToolUse"].as_array().unwrap().len(),
            1,
            "config must stay untouched when the sidecar is unwritable"
        );
    }

    #[test]
    fn legacy_in_file_stash_is_still_reenableable() {
        let entry = serde_json::json!({"hooks":[{"command":"first"}]});
        let path = crate::test_env::temp_file(
            "toggler-legacy-stash",
            "settings.json",
            serde_json::json!({
                "hooks":{"PreToolUse":[]},
                "_agentswitch_disabled":"broken"
            })
            .to_string()
            .as_bytes(),
        );
        let mut item = hook_item(path, &entry, "first");
        item.state = ItemState::Disabled;
        item.hook_loc = Some(HookLoc {
            section: "hooks".into(),
            event: "_stashed_PreToolUse".into(),
            order: 0,
            hook_name: "first".into(),
            fingerprint: hook_fingerprint(&entry),
        });
        let error = toggle_item(&mut item).unwrap_err().to_string();
        assert!(error.contains("_agentswitch_disabled is not an object"));
    }

    #[test]
    fn failed_stash_rolls_back_the_sidecar() {
        let entry = serde_json::json!({"hooks":[{"command":"first"}]});
        let path = crate::test_env::temp_file(
            "toggler-stash-order",
            "settings.json",
            serde_json::json!({"hooks":{"PreToolUse":[entry.clone()]}})
                .to_string()
                .as_bytes(),
        );
        std::fs::create_dir(path.with_file_name("settings.json.bak")).unwrap();
        let mut item = hook_item(path.clone(), &entry, "first");

        assert!(toggle_item(&mut item).is_err());
        assert!(
            !sidecar_path(&path).exists(),
            "a failed config commit must not leave the entry stranded in the sidecar"
        );
        let doc = read_doc(&path);
        assert_eq!(doc["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn failed_unstash_keeps_the_hook_recoverable() {
        let entry = serde_json::json!({"hooks":[{"command":"first"}]});
        let path = crate::test_env::temp_file(
            "toggler-unstash-order",
            "settings.json",
            serde_json::json!({"hooks":{"PreToolUse":[]}})
                .to_string()
                .as_bytes(),
        );
        std::fs::write(
            sidecar_path(&path),
            serde_json::json!({"PreToolUse":[entry.clone()]}).to_string(),
        )
        .unwrap();
        std::fs::create_dir(path.with_file_name("settings.json.bak")).unwrap();
        let mut item = hook_item(path.clone(), &entry, "first");
        item.state = ItemState::Disabled;
        item.hook_loc = Some(HookLoc {
            section: "hooks".into(),
            event: "_stashed_PreToolUse".into(),
            order: 0,
            hook_name: "first".into(),
            fingerprint: hook_fingerprint(&entry),
        });

        assert!(toggle_item(&mut item).is_err());
        let stash = read_doc(sidecar_path(&path));
        assert_eq!(
            stash["PreToolUse"][0]["hooks"][0]["command"], "first",
            "the hook must survive in the sidecar when the restore commit fails"
        );
        let doc = read_doc(&path);
        assert_eq!(
            doc["hooks"]["PreToolUse"].as_array().unwrap().len(),
            0,
            "config must be reverted to its pre-toggle state"
        );
    }

    #[test]
    fn failed_zcode_toggle_leaves_state_unchanged() {
        let first =
            serde_json::json!({"matcher":"Bash","hooks":[{"type":"process","command":"check"}]});
        let path = crate::test_env::temp_file(
            "toggler-zcode-state",
            "settings.json",
            serde_json::json!({"hooks":{"enabled":true,"events":{"PreToolUse":[first.clone()]}}})
                .to_string()
                .as_bytes(),
        );
        std::fs::create_dir(path.with_file_name("settings.json.bak")).unwrap();
        let mut item = ConfigItem::new("check", ItemKind::Hook, path, ProviderId::Zcode);
        item.hook_loc = Some(HookLoc {
            section: "hooks/events".into(),
            event: "PreToolUse".into(),
            order: 0,
            hook_name: "check".into(),
            fingerprint: hook_fingerprint(&first),
        });

        assert!(toggle_item(&mut item).is_err());
        assert!(
            item.state.is_enabled(),
            "state must not change when the commit fails"
        );
    }

    #[test]
    fn filesystem_agent_toggle_renames_and_restores_directory() {
        let path =
            crate::test_env::temp_file("toggler-agent-dir", "settings.json", "agent".as_bytes());
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
        let mut path = crate::test_env::temp_file(
            "toggler-codex-hook",
            "settings.json",
            "[hooks]\n".as_bytes(),
        );
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
        let mcp_path = crate::test_env::temp_file(
            "toggler-claude-mcp",
            "settings.json",
            r#"{"mcpServers":{"docs":{"type":"http","url":"https://example.test"}}}"#.as_bytes(),
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
        let settings = read_doc(settings_path);
        assert_eq!(
            settings["disabledMcpjsonServers"],
            serde_json::json!(["docs"])
        );
        assert!(settings.get("enabledMcpjsonServers").is_none());
    }

    #[test]
    fn antigravity_mcp_toggle_uses_disabled_flag() {
        let path = crate::test_env::temp_file(
            "toggler-antigravity-mcp",
            "settings.json",
            r#"{"mcpServers":{"docs":{"command":"server"}}}"#.as_bytes(),
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
        let config = read_doc(path);
        assert_eq!(config["mcpServers"]["docs"]["disabled"], true);
    }

    #[test]
    fn zcode_hook_toggle_uses_native_enabled_flag() {
        let first =
            serde_json::json!({"matcher":"Bash","hooks":[{"type":"process","command":"check"}]});
        let path = crate::test_env::temp_file(
            "toggler-zcode-hook",
            "settings.json",
            serde_json::json!({"hooks":{"enabled":true,"events":{"PreToolUse":[first.clone()]}}})
                .to_string()
                .as_bytes(),
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
        let doc = read_doc(&path);
        let entry = &doc["hooks"]["events"]["PreToolUse"][0];
        assert_eq!(entry["enabled"], serde_json::json!(false));
        assert_eq!(entry["matcher"], "Bash", "entry must stay in place");

        toggle_item(&mut item).unwrap();
        assert_eq!(item.state, ItemState::Enabled);
        let doc = read_doc(&path);
        assert!(doc["hooks"]["events"]["PreToolUse"][0]
            .get("enabled")
            .is_none());
    }

    #[test]
    fn json_stash_enable_rejects_a_live_server_with_the_same_name() {
        let path = crate::test_env::temp_file(
            "toggler-zcode-mcp-collision",
            "settings.json",
            r#"{"mcp":{"servers":{"docs":{"type":"stdio","command":"live"}}},"_disabled_mcp_servers":{"docs":{"type":"stdio","command":"stashed"}}}"#
                .as_bytes(),
        );
        let mut item = ConfigItem::new("docs", ItemKind::Mcp, path.clone(), ProviderId::Zcode);
        item.state = ItemState::Disabled;
        item.toggle_spec = Some(ToggleSpec::JsonStash {
            section: "mcp/servers".into(),
            name: "docs".into(),
        });

        let error = toggle_item(&mut item).unwrap_err().to_string();
        assert!(
            error.contains("already has a conflicting definition"),
            "actual: {error}"
        );
        let doc = read_doc(&path);
        assert_eq!(
            doc["mcp"]["servers"]["docs"]["command"], "live",
            "live definition must stay untouched"
        );
        assert_eq!(
            doc["_disabled_mcp_servers"]["docs"]["command"], "stashed",
            "stashed definition must stay untouched"
        );
        assert_eq!(item.state, ItemState::Disabled);
    }

    #[test]
    fn json_stash_disable_rejects_a_stash_entry_with_the_same_name() {
        let path = crate::test_env::temp_file(
            "toggler-zcode-mcp-disable-collision",
            "settings.json",
            r#"{"mcp":{"servers":{"docs":{"type":"stdio","command":"live"}}},"_disabled_mcp_servers":{"docs":{"type":"stdio","command":"stashed"}}}"#
                .as_bytes(),
        );
        let mut item = ConfigItem::new("docs", ItemKind::Mcp, path.clone(), ProviderId::Zcode);
        item.toggle_spec = Some(ToggleSpec::JsonStash {
            section: "mcp/servers".into(),
            name: "docs".into(),
        });

        let error = toggle_item(&mut item).unwrap_err().to_string();
        assert!(
            error.contains("already has a conflicting definition"),
            "actual: {error}"
        );
        let doc = read_doc(&path);
        assert_eq!(
            doc["mcp"]["servers"]["docs"]["command"], "live",
            "live definition must stay untouched"
        );
        assert_eq!(
            doc["_disabled_mcp_servers"]["docs"]["command"], "stashed",
            "stash must stay untouched"
        );
        assert_eq!(item.state, ItemState::Enabled);
    }

    #[test]
    fn zcode_duplicate_hook_toggle_targets_the_selected_entry() {
        let entry =
            serde_json::json!({"matcher":"Bash","hooks":[{"type":"process","command":"check"}]});
        let path = crate::test_env::temp_file(
            "toggler-zcode-duplicate-hook",
            "settings.json",
            serde_json::json!({"hooks":{"enabled":true,"events":{"PreToolUse":[
                entry.clone(), entry.clone()
            ]}}})
            .to_string()
            .as_bytes(),
        );
        let mut item = ConfigItem::new("check", ItemKind::Hook, path.clone(), ProviderId::Zcode);
        item.hook_loc = Some(HookLoc {
            section: "hooks/events".into(),
            event: "PreToolUse".into(),
            order: 1,
            hook_name: "check".into(),
            fingerprint: hook_fingerprint(&entry),
        });

        toggle_item(&mut item).unwrap();
        assert_eq!(item.state, ItemState::Disabled);
        let doc = read_doc(&path);
        assert!(
            doc["hooks"]["events"]["PreToolUse"][0]
                .get("enabled")
                .is_none(),
            "the unselected twin must stay enabled"
        );
        assert_eq!(
            doc["hooks"]["events"]["PreToolUse"][1]["enabled"],
            serde_json::json!(false)
        );
    }

    #[test]
    fn zcode_mcp_stash_moves_servers_out_of_mcp_servers() {
        let path = crate::test_env::temp_file(
            "toggler-zcode-mcp",
            "settings.json",
            r#"{"mcp":{"servers":{"docs":{"type":"stdio","command":"ctx"}}}}"#.as_bytes(),
        );
        let mut item = ConfigItem::new("docs", ItemKind::Mcp, path.clone(), ProviderId::Zcode);
        item.toggle_spec = Some(ToggleSpec::JsonStash {
            section: "mcp/servers".into(),
            name: "docs".into(),
        });

        toggle_item(&mut item).unwrap();
        let doc = read_doc(&path);
        assert!(doc["mcp"]["servers"].get("docs").is_none());
        assert_eq!(
            doc["_disabled_mcp_servers"]["docs"]["command"], "ctx",
            "disabled server keeps its definition"
        );

        toggle_item(&mut item).unwrap();
        let doc = read_doc(&path);
        assert_eq!(doc["mcp"]["servers"]["docs"]["command"], "ctx");
        assert!(doc.get("_disabled_mcp_servers").is_none());
    }

    #[test]
    fn reenabling_a_middle_hook_restores_its_original_order() {
        let a = serde_json::json!({"matcher":"A","hooks":[{"type":"command","command":"a"}]});
        let b = serde_json::json!({"matcher":"B","hooks":[{"type":"command","command":"b"}]});
        let c = serde_json::json!({"matcher":"C","hooks":[{"type":"command","command":"c"}]});
        let path = crate::test_env::temp_file(
            "toggler-order",
            "settings.json",
            serde_json::json!({"hooks":{"PostToolUse":[a.clone(), b.clone(), c.clone()]}})
                .to_string()
                .as_bytes(),
        );
        let mut middle = post_tool_use_item(path.clone(), &b, 1);
        toggle_item(&mut middle).unwrap();

        let doc = read_doc(&path);
        assert_eq!(doc["hooks"]["PostToolUse"].as_array().unwrap().len(), 2);

        toggle_item(&mut middle).unwrap();
        let doc = read_doc(&path);
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
        let path = crate::test_env::temp_file("toggler-order-shift", "settings.json", serde_json::json!({"hooks":{"PostToolUse":[a.clone(), b.clone(), c.clone(), d.clone()]}})
                .to_string().as_bytes());
        let mut first = post_tool_use_item(path.clone(), &a, 0);
        toggle_item(&mut first).unwrap();
        let mut third = post_tool_use_item(path.clone(), &c, 2);
        toggle_item(&mut third).unwrap();

        toggle_item(&mut third).unwrap();
        let doc = read_doc(&path);
        let restored = doc["hooks"]["PostToolUse"].as_array().unwrap();
        let matchers: Vec<_> = restored
            .iter()
            .map(|entry| entry["matcher"].as_str().unwrap())
            .collect();
        assert_eq!(matchers, ["B", "C", "D"]);

        toggle_item(&mut first).unwrap();
        let doc = read_doc(&path);
        let restored = doc["hooks"]["PostToolUse"].as_array().unwrap();
        let matchers: Vec<_> = restored
            .iter()
            .map(|entry| entry["matcher"].as_str().unwrap())
            .collect();
        assert_eq!(matchers, ["A", "B", "C", "D"]);
    }

    #[test]
    fn zcode_legacy_stash_items_are_reenableable() {
        let entry =
            serde_json::json!({"matcher":"Bash","hooks":[{"type":"command","command":"check"}]});
        let path = crate::test_env::temp_file(
            "toggler-zcode-stash",
            "settings.json",
            serde_json::json!({"hooks":{"enabled":true,"events":{"PreToolUse":[]}}})
                .to_string()
                .as_bytes(),
        );
        std::fs::write(
            sidecar_path(&path),
            serde_json::json!({"PreToolUse":[entry.clone()]}).to_string(),
        )
        .unwrap();
        let mut item = ConfigItem::new("check", ItemKind::Hook, path.clone(), ProviderId::Zcode);
        item.state = ItemState::Disabled;
        item.hook_loc = Some(HookLoc {
            section: "hooks/events".into(),
            event: "_stashed_PreToolUse".into(),
            order: 0,
            hook_name: "check".into(),
            fingerprint: stash_entry_fingerprint(&entry),
        });

        toggle_item(&mut item).unwrap();
        assert_eq!(item.state, ItemState::Enabled);
        let doc = read_doc(&path);
        assert_eq!(
            doc["hooks"]["events"]["PreToolUse"][0]["matcher"], "Bash",
            "the stashed entry must be restored into its native event array"
        );
        assert!(!sidecar_path(&path).exists(), "empty sidecar is removed");
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
    fn toml_root_inline_table_toggles_into_a_regular_table() {
        let path = crate::test_env::temp_file(
            "toggler-toml-inline-root",
            "config.toml",
            "mcp_servers = { docs = { command = \"docs\" } }\n".as_bytes(),
        );
        let mut item = ConfigItem::new("docs", ItemKind::Mcp, path.clone(), ProviderId::Codex);
        item.toggle_spec = Some(ToggleSpec::TomlFlag {
            section: "mcp_servers".into(),
            name: "docs".into(),
            flag: "enabled".into(),
            enabled_value: true,
            disabled_value: false,
        });

        toggle_item(&mut item).unwrap();
        let doc: toml::Value = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(doc["mcp_servers"]["docs"]["enabled"].as_bool(), Some(false));
        assert_eq!(doc["mcp_servers"]["docs"]["command"].as_str(), Some("docs"));
        assert_eq!(item.state, ItemState::Disabled);

        toggle_item(&mut item).unwrap();
        let doc: toml::Value = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(doc["mcp_servers"]["docs"]["enabled"].as_bool(), Some(true));
        assert_eq!(doc["mcp_servers"]["docs"]["command"].as_str(), Some("docs"));
        assert_eq!(item.state, ItemState::Enabled);
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
