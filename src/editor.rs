use anyhow::{Context, Result};
use std::path::PathBuf;

#[derive(Default)]
pub struct EditorState {
    pub path: Option<PathBuf>,
    pub content: String,
    pub original: String,
    pub dirty: bool,
    pub error: Option<String>,
}

impl EditorState {
    pub fn open(&mut self, path: PathBuf) -> Result<()> {
        if self.dirty {
            return Err(anyhow::anyhow!(
                "unsaved changes in {}; save or discard them first",
                self.filename()
            ));
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("could not open {}", path.display()))?;
        self.original = text.clone();
        self.content = text;
        self.path = Some(path);
        self.dirty = false;
        self.error = None;
        Ok(())
    }
    pub fn close(&mut self) {
        *self = Self::default();
    }
    pub fn save(&mut self) -> Result<()> {
        let Some(path) = self.path.clone() else {
            return Ok(());
        };
        let snapshot =
            crate::config_store::Snapshot::read_or(&path, self.original.as_bytes())?;
        if snapshot.text()? != self.original {
            anyhow::bail!(
                "{} changed on disk since it was opened; revert or reopen before saving",
                path.display()
            );
        }
        snapshot
            .commit(self.content.as_bytes())
            .with_context(|| format!("saving {}", path.display()))?;
        self.original = self.content.clone();
        self.dirty = false;
        self.error = None;
        Ok(())
    }
    pub fn revert(&mut self) {
        self.content = self.original.clone();
        self.dirty = false;
        self.error = None;
    }
    pub fn is_open(&self) -> bool {
        self.path.is_some()
    }
    pub fn update_dirty(&mut self) {
        self.dirty = self.content != self.original;
    }
    pub fn filename(&self) -> &str {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("agentswitch-editor-{name}-{nonce}.md"))
    }

    #[test]
    fn open_fails_without_clobbering_an_open_document() {
        let missing = temp_path("missing");
        let mut editor = EditorState::default();
        let existing = temp_path("existing");
        std::fs::write(&existing, "real doc").unwrap();
        editor.open(existing).unwrap();
        editor.content = "typed text".into();
        editor.update_dirty();

        let error = editor.open(missing).unwrap_err().to_string();
        assert!(error.contains("unsaved changes"));
        assert_eq!(editor.content, "typed text");
    }

    #[test]
    fn reopening_the_same_path_while_dirty_does_not_discard_edits() {
        let path = temp_path("same");
        std::fs::write(&path, "on disk").unwrap();
        let mut editor = EditorState::default();
        editor.open(path.clone()).unwrap();
        editor.content = "typed text".into();
        editor.update_dirty();

        let error = editor.open(path).unwrap_err().to_string();
        assert!(error.contains("unsaved changes"));
        assert_eq!(editor.content, "typed text");
    }

    #[test]
    fn save_refuses_to_overwrite_external_changes() {
        let path = temp_path("external");
        std::fs::write(&path, "original").unwrap();
        let mut editor = EditorState::default();
        editor.open(path.clone()).unwrap();
        editor.content = "mine".into();
        editor.update_dirty();
        std::fs::write(&path, "changed elsewhere").unwrap();

        assert!(editor.save().is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "changed elsewhere");

        std::fs::write(&path, "original").unwrap();
        editor.save().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "mine");
    }

    #[test]
    fn save_writes_a_compatible_backup() {
        let path = temp_path("backup");
        std::fs::write(&path, "original").unwrap();
        let mut editor = EditorState::default();
        editor.open(path.clone()).unwrap();
        editor.content = "mine".into();
        editor.update_dirty();

        editor.save().unwrap();
        let backup = crate::config_store::backup_path(&path);
        assert_eq!(std::fs::read_to_string(backup).unwrap(), "original");
    }
}
