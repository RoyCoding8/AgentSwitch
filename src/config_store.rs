use anyhow::{Context, Result};
use atomic_write_file::AtomicWriteFile;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Snapshot {
    path: PathBuf,
    bytes: Vec<u8>,
    existed: bool,
}

impl Snapshot {
    pub fn read(path: &Path) -> Result<Self> {
        Ok(Self {
            path: path.to_path_buf(),
            bytes: fs::read(path).with_context(|| format!("read {}", path.display()))?,
            existed: true,
        })
    }

    pub fn read_or(path: &Path, default: &[u8]) -> Result<Self> {
        if path.exists() {
            Self::read(path)
        } else {
            Ok(Self {
                path: path.to_path_buf(),
                bytes: default.to_vec(),
                existed: false,
            })
        }
    }

    pub fn text(&self) -> Result<&str> {
        std::str::from_utf8(&self.bytes)
            .with_context(|| format!("{} is not valid UTF-8", self.path.display()))
    }

    pub fn commit(&self, bytes: &[u8]) -> Result<()> {
        self.verify_current()?;
        if self.existed {
            backup_bytes(&self.path, &self.bytes)?;
        } else if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        atomic_write(&self.path, bytes)
    }

    pub fn verify_current(&self) -> Result<()> {
        if !self.existed {
            if self.path.exists() {
                anyhow::bail!(
                    "{} was created after it was checked; refresh and try again",
                    self.path.display()
                );
            }
            return Ok(());
        }
        let current = fs::read(&self.path)
            .with_context(|| format!("re-read {} before writing", self.path.display()))?;
        if current != self.bytes {
            anyhow::bail!(
                "{} changed after it was read; refresh and try again",
                self.path.display()
            );
        }
        Ok(())
    }
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = AtomicWriteFile::options()
        .open(path)
        .with_context(|| format!("open atomic writer for {}", path.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("write temporary contents for {}", path.display()))?;
    file.commit()
        .with_context(|| format!("replace {} atomically", path.display()))
}

pub fn move_path(source: &Path, target: &Path) -> Result<()> {
    if target.exists() {
        anyhow::bail!("destination already exists: {}", target.display());
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    match fs::rename(source, target) {
        Ok(()) => Ok(()),
        Err(rename_error) => {
            let staged = staged_path(target);
            // A previous failed attempt may have left a stale staging tree;
            // merging into it would weaken the copy verification below.
            cleanup(&staged);
            let copy_result = if source.is_dir() {
                copy_dir(source, &staged)
            } else {
                fs::copy(source, &staged)
                    .map(|_| ())
                    .map_err(anyhow::Error::from)
            };
            if let Err(error) = copy_result {
                cleanup(&staged);
                return Err(error).with_context(|| {
                    format!(
                        "move {} to {} after rename failed: {rename_error}",
                        source.display(),
                        target.display()
                    )
                });
            }
            if let Err(error) = verify_copy(source, &staged) {
                cleanup(&staged);
                return Err(error).with_context(|| {
                    format!(
                        "move {} to {} after rename failed: {rename_error}",
                        source.display(),
                        target.display()
                    )
                });
            }
            fs::rename(&staged, target)
                .with_context(|| format!("commit staged move to {}", target.display()))?;
            let remove_result = if source.is_dir() {
                fs::remove_dir_all(source)
            } else {
                fs::remove_file(source)
            };
            if let Err(remove_error) = remove_result {
                let undo = if target.is_dir() {
                    fs::remove_dir_all(target)
                } else {
                    fs::remove_file(target)
                };
                if let Err(undo_error) = undo {
                    anyhow::bail!(
                        "copied {} to {} but could not remove the source ({remove_error}) or undo the target ({undo_error})",
                        source.display(),
                        target.display()
                    );
                }
                return Err(remove_error).with_context(|| {
                    format!(
                        "copied {} to {} but could not remove the source; target was removed",
                        source.display(),
                        target.display()
                    )
                });
            }
            Ok(())
        }
    }
}

pub fn backup_path(path: &Path) -> PathBuf {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("bak");
    path.with_extension(format!("{extension}.bak"))
}

fn backup_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    let backup = backup_path(path);
    atomic_write(&backup, bytes).with_context(|| format!("back up {}", path.display()))
}

fn staged_path(target: &Path) -> PathBuf {
    let mut name = target.as_os_str().to_os_string();
    name.push(".agentswitch-moving");
    PathBuf::from(name)
}

fn copy_dir(source: &Path, target: &Path) -> Result<()> {
    fs::create_dir_all(target)?;
    // follow_links(false): a symlink loop inside a config tree must not
    // recurse forever; symlinked files are copied by content below.
    for entry in walkdir::WalkDir::new(source)
        .follow_links(false)
        .min_depth(1)
    {
        let entry = entry.with_context(|| format!("walk {}", source.display()))?;
        let relative = entry.path().strip_prefix(source)?;
        let dest = target.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&dest)?;
        } else {
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(entry.path(), &dest)?;
        }
    }
    Ok(())
}

fn verify_copy(source: &Path, target: &Path) -> Result<()> {
    if source.is_dir() {
        // Entry-count comparison passes when a copy truncates one file and
        // creates an extra one; compare every file's relative path and length.
        let files = |root: &Path| -> Result<std::collections::HashMap<String, u64>> {
            let mut map = std::collections::HashMap::new();
            for entry in walkdir::WalkDir::new(root).follow_links(false).min_depth(1) {
                let entry = entry.with_context(|| format!("walk {}", root.display()))?;
                if entry.file_type().is_dir() {
                    continue;
                }
                let relative = entry
                    .path()
                    .strip_prefix(root)?
                    .to_string_lossy()
                    .into_owned();
                map.insert(relative, fs::metadata(entry.path())?.len());
            }
            Ok(map)
        };
        if files(source)? != files(target)? {
            anyhow::bail!("staged directory copy is incomplete");
        }
    } else if fs::metadata(source)?.len() != fs::metadata(target)?.len() {
        anyhow::bail!("staged file copy has the wrong size");
    }
    Ok(())
}

fn cleanup(path: &Path) {
    if path.is_dir() {
        let _ = fs::remove_dir_all(path);
    } else {
        let _ = fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_file(name: &str, content: &[u8]) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("agentswitch-store-{name}-{nonce}"));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn commit_replaces_file_and_preserves_backup() {
        let path = temp_file("commit", b"old");
        let snapshot = Snapshot::read(&path).unwrap();
        snapshot.commit(b"new").unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert_eq!(fs::read(path.with_extension("json.bak")).unwrap(), b"old");
    }

    #[test]
    fn commit_rejects_external_edits() {
        let path = temp_file("stale", b"old");
        let snapshot = Snapshot::read(&path).unwrap();
        fs::write(&path, b"external").unwrap();

        let error = snapshot.commit(b"new").unwrap_err().to_string();
        assert!(error.contains("changed after it was read"));
        assert_eq!(fs::read(path).unwrap(), b"external");
    }

    #[test]
    fn commit_can_create_a_missing_file_atomically() {
        let path = temp_file("missing-parent", b"placeholder");
        fs::remove_file(&path).unwrap();
        let snapshot = Snapshot::read_or(&path, b"{}").unwrap();
        snapshot.commit(b"{\"enabled\":true}").unwrap();
        assert_eq!(fs::read(path).unwrap(), b"{\"enabled\":true}");
    }

    #[test]
    fn move_rejects_existing_destination() {
        let source = temp_file("move-source", b"source");
        let target = temp_file("move-target", b"target");
        let error = move_path(&source, &target).unwrap_err().to_string();
        assert!(error.contains("destination already exists"));
        assert_eq!(fs::read(source).unwrap(), b"source");
        assert_eq!(fs::read(target).unwrap(), b"target");
    }
}
