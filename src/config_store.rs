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
            cleanup(&staged);
            let copy_result = if source.is_dir() {
                copy_dir(source, &staged)
            } else {
                fs::copy(source, &staged)
                    .map(|_| ())
                    .map_err(anyhow::Error::from)
            };
            if let Err(error) = copy_result.and_then(|()| verify_copy(source, &staged)) {
                cleanup(&staged);
                return Err(error).with_context(|| {
                    format!(
                        "move {} to {} after rename failed: {rename_error}",
                        source.display(),
                        target.display()
                    )
                });
            }
            if let Err(error) = fs::rename(&staged, target) {
                cleanup(&staged);
                return Err(error)
                    .with_context(|| format!("commit staged move to {}", target.display()));
            }
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
    if backup.exists() && !plausible(path, bytes) {
        return Ok(());
    }
    atomic_write(&backup, bytes).with_context(|| format!("back up {}", path.display()))
}

pub(crate) fn jsonc_options() -> jsonc_parser::ParseOptions {
    jsonc_parser::ParseOptions {
        allow_comments: true,
        allow_trailing_commas: true,
        allow_loose_object_property_names: false,
        allow_missing_commas: false,
        allow_single_quoted_strings: false,
        allow_hexadecimal_numbers: false,
        allow_unary_plus_numbers: false,
    }
}

pub(crate) fn parse_json(path: &Path, text: &str) -> Result<serde_json::Value> {
    if path.extension().is_some_and(|ext| ext == "jsonc") {
        Ok(jsonc_parser::parse_to_serde_value(text, &jsonc_options())?)
    } else {
        Ok(serde_json::from_str(text)?)
    }
}

fn plausible(path: &Path, bytes: &[u8]) -> bool {
    match path.extension().and_then(|value| value.to_str()) {
        Some("json" | "jsonc") => {
            let body = bytes
                .strip_prefix([0xEF, 0xBB, 0xBF].as_slice())
                .unwrap_or(bytes);
            std::str::from_utf8(body)
                .ok()
                .is_some_and(|text| parse_json(path, text).is_ok())
        }
        Some("toml") => std::str::from_utf8(bytes)
            .ok()
            .is_some_and(|text| text.parse::<toml::Table>().is_ok()),
        _ => true,
    }
}

fn staged_path(target: &Path) -> PathBuf {
    let mut name = target.as_os_str().to_os_string();
    name.push(".agentswitch-moving");
    PathBuf::from(name)
}

fn copy_dir(source: &Path, target: &Path) -> Result<()> {
    fs::create_dir_all(target)?;
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
        let listing = |root: &Path| -> Result<std::collections::HashMap<String, u64>> {
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
        let right = listing(target)?;
        if listing(source)? != right {
            anyhow::bail!("staged directory copy is incomplete");
        }
        for relative in right.keys() {
            if !same_content(&source.join(relative), &target.join(relative))? {
                anyhow::bail!("staged directory copy has the wrong content");
            }
        }
    } else if !same_content(source, target)? {
        anyhow::bail!("staged file copy has the wrong content");
    }
    Ok(())
}

fn same_content(a: &Path, b: &Path) -> Result<bool> {
    let mut left = std::io::BufReader::new(fs::File::open(a)?);
    let mut right = std::io::BufReader::new(fs::File::open(b)?);
    loop {
        let mut chunk_a = [0u8; 16 * 1024];
        let mut chunk_b = [0u8; 16 * 1024];
        let na = fill(&mut left, &mut chunk_a)?;
        let nb = fill(&mut right, &mut chunk_b)?;
        if na != nb || chunk_a[..na] != chunk_b[..nb] {
            return Ok(false);
        }
        if na == 0 {
            return Ok(true);
        }
    }
}

fn fill(reader: &mut impl std::io::Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
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

    #[test]
    fn jsonc_backups_rotate_valid_comments_but_keep_last_good_on_corruption() {
        let original = b"{\"v\": 1}";
        let commented = b"{/* saved comment */\"v\": 2,}";
        let path = crate::test_env::temp_file("store-jsonc-backup", "config.jsonc", original);
        Snapshot::read(&path).unwrap().commit(commented).unwrap();
        Snapshot::read(&path)
            .unwrap()
            .commit(b"{\"v\": 3}")
            .unwrap();
        assert_eq!(fs::read(backup_path(&path)).unwrap(), commented);
        fs::write(&path, b"{/* unterminated").unwrap();
        Snapshot::read(&path)
            .unwrap()
            .commit(b"{\"v\": 4}")
            .unwrap();
        assert_eq!(fs::read(backup_path(&path)).unwrap(), commented);
        assert_eq!(fs::read(&path).unwrap(), b"{\"v\": 4}");
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn jsonc_accepts_only_comments_and_trailing_commas_beyond_json() {
        let path = Path::new("opencode.jsonc");
        assert_eq!(
            parse_json(path, "{/* note */\"v\": [1,],}").unwrap(),
            serde_json::json!({"v": [1]})
        );
        for text in [
            "{v: 1}",
            "{'v': 1}",
            "{\"v\": [1 2]}",
            "{\"v\": 0xff}",
            "{\"v\": +1}",
            "{\"v\": 1} /* unfinished",
            "{\"v\": [,]}",
        ] {
            assert!(
                parse_json(path, text).is_err(),
                "accepted malformed JSONC: {text}"
            );
        }
        assert!(parse_json(Path::new("config.json"), "{/* note */\"v\": 1,}").is_err());
    }

    #[test]
    fn commit_replaces_file_and_preserves_backup() {
        let path = crate::test_env::temp_file("store-commit", "config.json", b"old");
        let snapshot = Snapshot::read(&path).unwrap();
        snapshot.commit(b"new").unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert_eq!(fs::read(path.with_extension("json.bak")).unwrap(), b"old");
    }

    #[test]
    fn commit_rejects_external_edits() {
        let path = crate::test_env::temp_file("store-stale", "config.json", b"old");
        let snapshot = Snapshot::read(&path).unwrap();
        fs::write(&path, b"external").unwrap();

        let error = snapshot.commit(b"new").unwrap_err().to_string();
        assert!(error.contains("changed after it was read"));
        assert_eq!(fs::read(path).unwrap(), b"external");
    }

    #[test]
    fn commit_keeps_the_last_valid_backup_over_corrupted_content() {
        let path = crate::test_env::temp_file("store-bak-guard", "config.json", b"{\"v\":1}");
        let snapshot = Snapshot::read(&path).unwrap();
        snapshot.commit(b"{\"v\":2}").unwrap();

        fs::write(&path, b"corrupted{").unwrap();
        let snapshot = Snapshot::read(&path).unwrap();
        snapshot.commit(b"{\"v\":3}").unwrap();

        let backup = fs::read(path.with_extension("json.bak")).unwrap();
        assert_eq!(
            std::str::from_utf8(&backup).unwrap(),
            "{\"v\":1}",
            "an externally corrupted snapshot must not overwrite the good backup"
        );
        assert_eq!(fs::read(&path).unwrap(), b"{\"v\":3}");
    }

    #[test]
    fn commit_still_rotates_backups_between_valid_saves() {
        let path = crate::test_env::temp_file("store-bak-rotate", "config.json", b"{\"v\":1}");
        let snapshot = Snapshot::read(&path).unwrap();
        snapshot.commit(b"{\"v\":2}").unwrap();
        let snapshot = Snapshot::read(&path).unwrap();
        snapshot.commit(b"{\"v\":3}").unwrap();
        let backup = fs::read(path.with_extension("json.bak")).unwrap();
        assert_eq!(std::str::from_utf8(&backup).unwrap(), "{\"v\":2}");
    }

    #[test]
    fn commit_can_create_a_missing_file_atomically() {
        let path =
            crate::test_env::temp_file("store-missing-parent", "config.json", b"placeholder");
        fs::remove_file(&path).unwrap();
        let snapshot = Snapshot::read_or(&path, b"{}").unwrap();
        snapshot.commit(b"{\"enabled\":true}").unwrap();
        assert_eq!(fs::read(path).unwrap(), b"{\"enabled\":true}");
    }

    #[test]
    fn move_rejects_existing_destination() {
        let source = crate::test_env::temp_file("store-move-source", "config.json", b"source");
        let target = crate::test_env::temp_file("store-move-target", "config.json", b"target");
        let error = move_path(&source, &target).unwrap_err().to_string();
        assert!(error.contains("destination already exists"));
        assert_eq!(fs::read(source).unwrap(), b"source");
        assert_eq!(fs::read(target).unwrap(), b"target");
    }
}
