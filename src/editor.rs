use std::path::PathBuf;

#[derive(Default)]
pub struct EditorState {
    pub path: Option<PathBuf>,
    pub content: String,
    pub original: String,
    pub dirty: bool,
}

impl EditorState {
    pub fn open(&mut self, path: PathBuf) {
        if let Ok(text) = std::fs::read_to_string(&path) {
            self.content = text.clone();
            self.original = text;
            self.path = Some(path);
            self.dirty = false;
        }
    }
    pub fn close(&mut self) {
        *self = Self::default();
    }
    pub fn save(&mut self) -> anyhow::Result<()> {
        if let Some(p) = &self.path {
            std::fs::write(p, &self.content)?;
            self.original = self.content.clone();
            self.dirty = false;
        }
        Ok(())
    }
    pub fn revert(&mut self) {
        self.content = self.original.clone();
        self.dirty = false;
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
