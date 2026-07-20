use crate::config_store::{atomic_write, backup_path, move_path};
use crate::toggler;
use crate::types::{ConfigItem, ToggleSpec};
use anyhow::Result;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub struct BatchOutcome {
    pub toggled: usize,
    pub error: Option<String>,
    pub rollback_errors: Vec<String>,
}

struct Recovery {
    index: usize,
    item: ConfigItem,
    files: HashMap<PathBuf, Option<Vec<u8>>>,
}

pub fn toggle(items: &mut [ConfigItem], indices: &[usize]) -> BatchOutcome {
    let recoveries = match capture(items, indices) {
        Ok(recoveries) => recoveries,
        Err(error) => {
            return BatchOutcome {
                toggled: 0,
                error: Some(error.to_string()),
                rollback_errors: vec![],
            };
        }
    };
    let mut toggled = 0;
    for &index in indices {
        match toggler::toggle_item(&mut items[index]) {
            Ok(()) => toggled += 1,
            Err(error) => {
                let rollback_errors = rollback(items, &recoveries[..toggled]);
                return BatchOutcome {
                    toggled,
                    error: Some(format!("{}: {error}", items[index].name)),
                    rollback_errors,
                };
            }
        }
    }
    BatchOutcome {
        toggled,
        error: None,
        rollback_errors: vec![],
    }
}

fn capture(items: &[ConfigItem], indices: &[usize]) -> Result<Vec<Recovery>> {
    indices
        .iter()
        .map(|&index| {
            let item = items[index].clone();
            let mut paths = vec![item.path.clone()];
            if let Some(ToggleSpec::StringLists { path, .. }) = &item.toggle_spec {
                paths.push(path.clone());
            }
            let mut files = HashMap::new();
            for path in paths {
                capture_file(&mut files, &path)?;
                capture_file(&mut files, &backup_path(&path))?;
            }
            Ok(Recovery { index, item, files })
        })
        .collect()
}

fn capture_file(files: &mut HashMap<PathBuf, Option<Vec<u8>>>, path: &Path) -> Result<()> {
    if files.contains_key(path) || path.is_dir() {
        return Ok(());
    }
    files.insert(
        path.to_path_buf(),
        if path.exists() {
            Some(fs::read(path)?)
        } else {
            None
        },
    );
    Ok(())
}

fn rollback(items: &mut [ConfigItem], recoveries: &[Recovery]) -> Vec<String> {
    let mut errors = Vec::new();
    for recovery in recoveries.iter().rev() {
        let current = &items[recovery.index];
        if current.path != recovery.item.path && current.path.exists() {
            if let Err(error) = move_path(&current.path, &recovery.item.path) {
                errors.push(format!("{}: {error}", recovery.item.name));
            }
        }
        let before = errors.len();
        for (path, bytes) in &recovery.files {
            let result = match bytes {
                Some(bytes) => atomic_write(path, bytes),
                None if path.exists() => fs::remove_file(path).map_err(anyhow::Error::from),
                None => Ok(()),
            };
            if let Err(error) = result {
                errors.push(format!("{}: {error}", path.display()));
            }
        }
        if errors.len() == before {
            items[recovery.index] = recovery.item.clone();
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ItemKind, ProviderId};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("agentswitch-batch-{nonce}"));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn failed_batch_restores_prior_rename_exactly() {
        let dir = temp_dir();
        let first_path = dir.join("first.md");
        let second_path = dir.join("second.md");
        fs::write(&first_path, "first").unwrap();
        fs::write(&second_path, "second").unwrap();
        fs::write(second_path.with_extension("md.disabled"), "collision").unwrap();
        let mut items = vec![
            ConfigItem::new(
                "first.md",
                ItemKind::Rule,
                first_path.clone(),
                ProviderId::Claude,
            ),
            ConfigItem::new("second.md", ItemKind::Rule, second_path, ProviderId::Claude),
        ];

        let outcome = toggle(&mut items, &[0, 1]);
        assert!(outcome.error.is_some());
        assert!(outcome.rollback_errors.is_empty());
        assert_eq!(fs::read_to_string(&first_path).unwrap(), "first");
        assert!(!first_path.with_extension("md.disabled").exists());
        assert!(items[0].state.is_enabled());
    }
}
