use crate::chat::{self, ChatSession};
use crate::diagnostics;
use crate::editor::EditorState;
use crate::hook_diag;
use crate::scanner;
use crate::toggler;
use crate::types::*;
use crate::ui;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Items,
    Hooks,
    Diff,
    Chats,
}

pub struct App {
    workspace: PathBuf,
    scope: Scope,
    providers: Vec<(ProviderId, bool)>,
    selected_provider: Option<ProviderId>,
    items: Vec<ConfigItem>,
    diff_rows: Vec<diagnostics::DiffRow>,
    diff_filter: diagnostics::DiffFilter,
    hook_rows: Vec<hook_diag::HookRow>,
    hook_filter: hook_diag::HookFilter,
    chat_sessions: Vec<ChatSession>,
    chat_trash: Vec<ChatSession>,
    chat_selection: HashSet<String>,
    chat_search: String,
    chat_trash_mode: bool,
    filter: FilterKind,
    view: View,
    editor: EditorState,
    status_msg: Option<String>,
    browse_requested: bool,
    first_frame: bool,
}

impl App {
    pub fn new() -> Self {
        let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let mut app = Self {
            workspace,
            scope: Scope::Project,
            providers: vec![],
            selected_provider: None,
            items: vec![],
            diff_rows: vec![],
            diff_filter: diagnostics::DiffFilter::All,
            hook_rows: vec![],
            hook_filter: hook_diag::HookFilter::All,
            chat_sessions: vec![],
            chat_trash: vec![],
            chat_selection: HashSet::new(),
            chat_search: String::new(),
            chat_trash_mode: false,
            filter: FilterKind::All,
            view: View::Items,
            editor: EditorState::default(),
            status_msg: None,
            browse_requested: false,
            first_frame: true,
        };
        app.refresh();
        app.rescan_chats();
        app
    }

    fn scan_root(&self) -> PathBuf {
        match self.scope {
            Scope::Project => self.workspace.clone(),
            Scope::Global => dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")),
        }
    }

    fn refresh(&mut self) {
        let root = self.scan_root();
        self.providers = ProviderId::ALL
            .iter()
            .map(|&id| (id, scanner::provider_exists(id, &root, self.scope)))
            .collect();
        // auto-select first detected
        if self.selected_provider.is_none()
            || !self
                .providers
                .iter()
                .any(|(id, d)| *d && Some(*id) == self.selected_provider)
        {
            self.selected_provider = self.providers.iter().find(|(_, d)| *d).map(|(id, _)| *id);
        }
        self.rescan_items();
    }

    fn rescan_items(&mut self) {
        let root = self.scan_root();
        self.items = match self.selected_provider {
            Some(id) => scanner::scan_provider(id, &root, self.scope),
            None => vec![],
        };
        self.diff_rows = match self.selected_provider {
            Some(id) => diagnostics::build(id, &self.workspace),
            None => vec![],
        };
        self.diff_filter = diagnostics::default_filter(&self.diff_rows);
        self.hook_rows = match self.selected_provider {
            Some(id) => hook_diag::build(id, &self.workspace),
            None => vec![],
        };
        self.hook_filter = hook_diag::default_filter(&self.hook_rows);
    }

    fn rescan_chats(&mut self) {
        self.chat_sessions = chat::scan_all(&self.workspace);
        self.chat_trash = chat::scan_trash();
        let keys: HashSet<_> = self
            .chat_sessions
            .iter()
            .chain(self.chat_trash.iter())
            .map(chat::session_key)
            .collect();
        self.chat_selection.retain(|key| keys.contains(key));
    }

    fn available_kinds(&self) -> Vec<ItemKind> {
        let mut seen = HashSet::new();
        self.items
            .iter()
            .filter_map(|i| {
                if seen.insert(i.kind) {
                    Some(i.kind)
                } else {
                    None
                }
            })
            .collect()
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.first_frame {
            ui::theme::apply(ctx);
            self.first_frame = false;
        }

        // browse dialog
        if self.browse_requested {
            self.browse_requested = false;
            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                self.workspace = path;
                self.refresh();
                self.rescan_chats();
            }
        }

        let old_scope = self.scope;
        let old_provider = self.selected_provider;

        // sidebar
        egui::SidePanel::left("sidebar")
            .min_width(170.0)
            .max_width(200.0)
            .frame(
                egui::Frame::NONE
                    .fill(ui::theme::BG_SIDEBAR)
                    .inner_margin(egui::Margin::same(8)),
            )
            .show(ctx, |ui_panel| {
                ui::sidebar::show(
                    ui_panel,
                    &self.providers,
                    &mut self.selected_provider,
                    &mut self.scope,
                    &self.workspace.to_string_lossy(),
                    &mut self.browse_requested,
                );
            });

        // bottom status
        egui::TopBottomPanel::bottom("status")
            .frame(
                egui::Frame::NONE
                    .fill(ui::theme::BG_SIDEBAR)
                    .inner_margin(egui::Margin::same(6)),
            )
            .show(ctx, |ui_panel| {
                ui::status_bar::show(ui_panel, &self.items, &self.providers);
                if let Some(msg) = &self.status_msg {
                    ui_panel.label(
                        egui::RichText::new(msg)
                            .font(ui::theme::small_font())
                            .color(ui::theme::YELLOW),
                    );
                }
            });

        // main content
        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(ui::theme::BG_PANEL)
                    .inner_margin(egui::Margin::same(16)),
            )
            .show(ctx, |ui_panel| {
                if self.editor.is_open() {
                    ui::editor_panel::show(ui_panel, &mut self.editor);
                } else if self.view == View::Chats {
                    self.show_chats(ui_panel);
                } else if let Some(provider) = self.selected_provider {
                    let root = self.scan_root();
                    let dir = scanner::provider_dir(provider, &root, self.scope);
                    ui_panel.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(provider.label())
                                .font(ui::theme::heading_font())
                                .color(ui::theme::TEXT_PRIMARY),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Open folder").clicked() {
                                open_path(&dir);
                            }
                            for md in instruction_files(provider, &root, &dir, self.scope) {
                                let label = md
                                    .file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .to_string();
                                let exists = md.exists();
                                let btn_text = if exists {
                                    label.clone()
                                } else {
                                    format!("+ {}", label)
                                };
                                let color = if exists {
                                    ui::theme::TEXT_ACCENT
                                } else {
                                    ui::theme::TEXT_DIM
                                };
                                if ui
                                    .button(
                                        egui::RichText::new(btn_text)
                                            .color(color)
                                            .font(ui::theme::small_font()),
                                    )
                                    .clicked()
                                {
                                    if !exists {
                                        if let Some(p) = md.parent() {
                                            let _ = std::fs::create_dir_all(p);
                                        }
                                        let _ = std::fs::write(
                                            &md,
                                            format!("# {} instructions\n", provider.label()),
                                        );
                                    }
                                    self.editor.open(md);
                                }
                            }
                        });
                    });
                    ui_panel.add_space(4.0);
                    ui_panel.horizontal(|ui| {
                        view_tab(ui, &mut self.view, View::Items, "Items");
                        view_tab(ui, &mut self.view, View::Hooks, "Hooks");
                        view_tab(ui, &mut self.view, View::Diff, "Diff");
                        view_tab(ui, &mut self.view, View::Chats, "Chats");
                        if self.view == View::Items {
                            let kinds = self.available_kinds();
                            ui::item_list::filter_tabs(ui, &mut self.filter, &kinds);
                        }
                    });
                    ui_panel.add_space(8.0);
                    match self.view {
                        View::Diff => {
                            let action = ui::diff_panel::show(
                                ui_panel,
                                &self.diff_rows,
                                &mut self.diff_filter,
                            );
                            if let Some(path) = action.open {
                                open_path(&path);
                            }
                            return;
                        }
                        View::Hooks => {
                            let action = ui::hooks_panel::show(
                                ui_panel,
                                &self.hook_rows,
                                &mut self.hook_filter,
                            );
                            if let Some(path) = action.open {
                                open_path(&path);
                            }
                            return;
                        }
                        View::Chats => {
                            self.show_chats(ui_panel);
                            return;
                        }
                        View::Items => {}
                    }
                    let result = ui::item_list::show(ui_panel, &self.items, self.filter);
                    if result.enable_all || result.disable_all {
                        let want_enabled = result.enable_all;
                        let indices: Vec<usize> = self
                            .items
                            .iter()
                            .enumerate()
                            .filter(|(_, it)| match self.filter {
                                FilterKind::All => true,
                                FilterKind::Specific(k) => it.kind == k,
                            })
                            .filter(|(_, it)| it.state.is_enabled() != want_enabled)
                            .map(|(i, _)| i)
                            .collect();
                        let (mut ok, mut errs) = (0usize, vec![]);
                        for idx in indices {
                            match toggler::toggle_item(&mut self.items[idx]) {
                                Ok(()) => ok += 1,
                                Err(e) => errs.push(format!("{}: {e}", self.items[idx].name)),
                            }
                        }
                        self.status_msg = Some(if errs.is_empty() {
                            format!(
                                "{} items {}",
                                ok,
                                if want_enabled { "enabled" } else { "disabled" }
                            )
                        } else {
                            format!("{ok} ok, {} errors: {}", errs.len(), errs.join("; "))
                        });
                        self.rescan_items();
                    } else if let Some(idx) = result.index {
                        if idx < self.items.len() {
                            match toggler::toggle_item(&mut self.items[idx]) {
                                Ok(()) => {
                                    self.status_msg =
                                        Some(format!("Toggled: {}", self.items[idx].name));
                                    self.rescan_items();
                                }
                                Err(e) => self.status_msg = Some(format!("Error: {e}")),
                            }
                        }
                    }
                    if let Some(idx) = result.edit {
                        if idx < self.items.len() && self.items[idx].editable {
                            self.editor.open(self.items[idx].path.clone());
                        }
                    }
                } else {
                    ui_panel.add_space(60.0);
                    ui_panel.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new("No providers detected")
                                .font(ui::theme::heading_font())
                                .color(ui::theme::TEXT_DIM),
                        );
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new(
                                "Select a workspace with AI agent config directories",
                            )
                            .font(ui::theme::body_font())
                            .color(ui::theme::TEXT_DIM),
                        );
                    });
                }
            });

        // refresh on scope/provider change
        if self.scope != old_scope {
            self.refresh();
        } else if self.selected_provider != old_provider {
            self.rescan_items();
            self.filter = FilterKind::All;
        }
        // Prevent being stuck in a view with no way to navigate away
        if self.selected_provider.is_none()
            && matches!(self.view, View::Items | View::Hooks | View::Diff)
        {
            self.view = View::Chats;
        }
    }
}

impl App {
    fn show_chats(&mut self, ui_panel: &mut egui::Ui) {
        ui_panel.horizontal(|ui| {
            if ui.button("Back").clicked() {
                if self.selected_provider.is_none() {
                    self.selected_provider = self
                        .providers
                        .iter()
                        .find(|(_, exists)| *exists)
                        .map(|(id, _)| *id);
                }
                self.view = View::Items;
            }
            ui.label(
                egui::RichText::new(if self.chat_trash_mode {
                    "Chats Trash"
                } else {
                    "Chats"
                })
                .font(ui::theme::heading_font())
                .color(ui::theme::TEXT_PRIMARY),
            );
        });
        ui_panel.add_space(4.0);
        let action = if self.chat_trash_mode {
            ui::chat_panel::show(
                ui_panel,
                &self.chat_trash,
                &mut self.chat_selection,
                &mut self.chat_search,
                true,
            )
        } else {
            ui::chat_panel::show(
                ui_panel,
                &self.chat_sessions,
                &mut self.chat_selection,
                &mut self.chat_search,
                false,
            )
        };
        if action.refresh {
            self.rescan_chats();
            self.status_msg = Some("Chats refreshed".into());
        }
        if action.toggle_trash {
            self.chat_trash_mode = !self.chat_trash_mode;
            self.chat_selection.clear();
        }
        if action.import {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("AgentSwitch chat/zip", &["json", "zip"])
                .set_directory(chat::exports_dir())
                .pick_file()
            {
                let project_dir = rfd::FileDialog::new()
                    .set_title("Associate with project directory (Cancel to skip)")
                    .pick_folder();
                let is_zip = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("zip"));
                if is_zip {
                    match chat::import_zip(&path, project_dir.as_deref()) {
                        Ok(report) => {
                            self.rescan_chats();
                            self.status_msg = Some(format!(
                                "{} chats imported, {} failed",
                                report.ok, report.failed
                            ));
                        }
                        Err(e) => self.status_msg = Some(format!("Import error: {e}")),
                    }
                } else {
                    match chat::import_archive(&path, project_dir.as_deref()) {
                        Ok(_) => {
                            self.rescan_chats();
                            self.status_msg = Some("Chat imported".into());
                        }
                        Err(e) => self.status_msg = Some(format!("Import error: {e}")),
                    }
                }
            }
        }
        if !action.export.is_empty() {
            let sessions: Vec<_> = action
                .export
                .iter()
                .filter_map(|idx| self.chat_sessions.get(*idx).cloned())
                .collect();
            let _ = std::fs::create_dir_all(chat::exports_dir());
            if sessions.len() == 1 {
                let session = &sessions[0];
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("AgentSwitch chat", &["json"])
                    .set_file_name(chat::suggested_export_name(session))
                    .set_directory(chat::exports_dir())
                    .save_file()
                {
                    match chat::export_session(session, &path) {
                        Ok(()) => self.status_msg = Some("Chat exported".into()),
                        Err(e) => self.status_msg = Some(format!("Export error: {e}")),
                    }
                }
            } else if !sessions.is_empty() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("AgentSwitch chats", &["zip"])
                    .set_file_name(chat::suggested_zip_export_name())
                    .set_directory(chat::exports_dir())
                    .save_file()
                {
                    match chat::export_sessions_zip(&sessions, &path) {
                        Ok(report) => {
                            self.status_msg = Some(format!(
                                "{} chats exported, {} failed",
                                report.ok, report.failed
                            ));
                        }
                        Err(e) => self.status_msg = Some(format!("Export error: {e}")),
                    }
                }
            }
        }
        if !action.trash.is_empty() {
            let mut ok = 0usize;
            let mut failed = 0usize;
            for idx in action.trash {
                match self
                    .chat_sessions
                    .get(idx)
                    .map(|session| chat::soft_delete(session, &self.workspace))
                {
                    Some(Ok(())) => ok += 1,
                    _ => failed += 1,
                }
            }
            self.chat_selection.clear();
            self.rescan_chats();
            self.status_msg = Some(format!("{ok} chats moved to Trash, {failed} failed"));
        }
        if !action.restore.is_empty() {
            let mut ok = 0usize;
            let mut failed = 0usize;
            let mut alternates = 0usize;
            for idx in action.restore {
                if let Some(session) = self.chat_trash.get(idx) {
                    let original = session
                        .trash_manifest
                        .as_ref()
                        .and_then(|p| std::fs::read_to_string(p).ok())
                        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                        .and_then(|v| {
                            v.get("original_path")
                                .and_then(|p| p.as_str())
                                .map(PathBuf::from)
                        });
                    match chat::restore_from_trash(session) {
                        Ok(path) => {
                            ok += 1;
                            if original.as_ref().is_some_and(|p| p != &path) {
                                alternates += 1;
                            }
                        }
                        Err(_) => failed += 1,
                    }
                }
            }
            self.chat_selection.clear();
            self.rescan_chats();
            self.status_msg = Some(format!(
                "{ok} chats restored, {failed} failed, {alternates} renamed"
            ));
        }
        if !action.delete_forever.is_empty() || !action.empty_visible.is_empty() {
            let indices = if action.delete_forever.is_empty() {
                action.empty_visible
            } else {
                action.delete_forever
            };
            let mut ok = 0usize;
            let mut failed = 0usize;
            for idx in indices {
                match self.chat_trash.get(idx).map(chat::delete_trash_forever) {
                    Some(Ok(())) => ok += 1,
                    _ => failed += 1,
                }
            }
            self.chat_selection.clear();
            self.rescan_chats();
            self.status_msg = Some(format!("{ok} trash chats deleted forever, {failed} failed"));
        }
    }
}

fn open_path(path: &std::path::Path) {
    let _ = if cfg!(windows) {
        std::process::Command::new("explorer").arg(path).spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(path).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(path).spawn()
    };
}

fn view_tab(ui: &mut egui::Ui, view: &mut View, value: View, label: &str) {
    let active = *view == value;
    if ui
        .selectable_label(
            active,
            egui::RichText::new(label)
                .font(ui::theme::small_font())
                .color(if active {
                    ui::theme::TEXT_ACCENT
                } else {
                    ui::theme::TEXT_DIM
                }),
        )
        .clicked()
    {
        *view = value;
    }
}

fn instruction_files(provider: ProviderId, root: &Path, dir: &Path, scope: Scope) -> Vec<PathBuf> {
    match (provider, scope) {
        (ProviderId::Claude, Scope::Project) => vec![root.join("CLAUDE.md")],
        (ProviderId::Claude, Scope::Global) => vec![dir.join("CLAUDE.md")],
        (ProviderId::Codex, Scope::Project) => vec![root.join("AGENTS.md")],
        (ProviderId::Codex, Scope::Global) => vec![dir.join("AGENTS.md")],

        (ProviderId::Antigravity, Scope::Project) => {
            vec![root.join("GEMINI.md"), root.join("AGENTS.md")]
        }
        (ProviderId::Antigravity, Scope::Global) => {
            let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
            let gemini_dir = home.join(".gemini");
            vec![gemini_dir.join("GEMINI.md"), gemini_dir.join("AGENTS.md")]
        }
        (ProviderId::Kiro, Scope::Project) => {
            vec![root.join(".kiro").join("steering").join("instructions.md")]
        }
        (ProviderId::Kiro, Scope::Global) => vec![dir.join("steering").join("instructions.md")],
        (ProviderId::OpenCode, Scope::Project) => vec![root.join("AGENTS.md")],
        (ProviderId::OpenCode, Scope::Global) => vec![dir.join("AGENTS.md")],
    }
}
