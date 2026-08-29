use crate::batch;
use crate::chat::{self, ChatSession};
use crate::diagnostics;
use crate::editor::EditorState;
use crate::hook_diag;
use crate::provider;
use crate::scanner;
use crate::toggler;
use crate::types::*;
use crate::ui;
use std::collections::HashSet;
use std::path::PathBuf;

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
    chat_delete_confirm: Option<(Vec<String>, bool)>,
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
            chat_delete_confirm: None,
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

    fn scan_root(&self) -> Option<PathBuf> {
        match self.scope {
            Scope::Project => Some(self.workspace.clone()),
            Scope::Global => provider::home_dir().ok(),
        }
    }

    fn refresh(&mut self) {
        let Some(root) = self.scan_root() else {
            self.providers.clear();
            self.selected_provider = None;
            self.items.clear();
            self.status_msg = Some("Cannot determine the user home directory".into());
            return;
        };
        self.providers = ProviderId::ALL
            .iter()
            .map(|&id| (id, scanner::provider_exists(id, &root, self.scope)))
            .collect();
        if self.selected_provider.is_none()
            || !self
                .providers
                .iter()
                .any(|(id, d)| *d && Some(*id) == self.selected_provider)
        {
            self.selected_provider = self.providers.iter().find(|(_, d)| *d).map(|(id, _)| *id);
        }
        self.filter = FilterKind::All;
        self.rescan_items();
    }

    fn rescan_items(&mut self) {
        let Some(root) = self.scan_root() else {
            self.items.clear();
            self.diff_rows.clear();
            self.hook_rows.clear();
            return;
        };
        self.items = match self.selected_provider {
            Some(id) => scanner::scan_provider(id, &root, self.scope),
            None => vec![],
        };
        self.diff_rows = match self.selected_provider {
            Some(id) => diagnostics::build(id, &self.workspace),
            None => vec![],
        };
        self.hook_rows = match self.selected_provider {
            Some(id) => hook_diag::build(id, &self.workspace),
            None => vec![],
        };
        if !self
            .diff_rows
            .iter()
            .any(|row| self.diff_filter.matches(row))
        {
            self.diff_filter = diagnostics::DiffFilter::All;
        }
        if !self
            .hook_rows
            .iter()
            .any(|row| self.hook_filter.matches(row))
        {
            self.hook_filter = hook_diag::HookFilter::All;
        }
    }

    fn rescan_chats(&mut self) {
        let filter = if self.view == View::Chats {
            self.selected_provider
        } else {
            None
        };
        self.chat_delete_confirm = None;
        self.chat_sessions = chat::scan_all(filter);
        self.chat_trash = chat::scan_trash(filter);
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
                } else if let Some(provider_id) = self.selected_provider {
                    let Some(root) = self.scan_root() else {
                        ui_panel.label("Cannot determine the user home directory");
                        return;
                    };
                    let Ok(dir) = scanner::provider_dir(provider_id, &root, self.scope) else {
                        ui_panel.label("Cannot resolve the provider configuration directory");
                        return;
                    };
                    let provider = provider_id;
                    ui_panel.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(provider.label())
                                .font(ui::theme::heading_font())
                                .color(ui::theme::TEXT_PRIMARY),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Open folder").clicked() {
                                self.open_and_report(&dir);
                            }
                            for md in provider::instruction_files(provider, &root, self.scope)
                                .unwrap_or_default()
                            {
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
                                        let created = md
                                            .parent()
                                            .map(std::fs::create_dir_all)
                                            .transpose()
                                            .and_then(|_| {
                                                std::fs::write(
                                                    &md,
                                                    format!(
                                                        "# {} instructions\n",
                                                        provider.label()
                                                    ),
                                                )
                                            });
                                        if let Err(error) = created {
                                            self.status_msg = Some(format!(
                                                "Could not create {}: {error}",
                                                md.display()
                                            ));
                                            return;
                                        }
                                    }
                                    self.editor.open(md).unwrap_or_else(|error| {
                                        self.status_msg = Some(format!("Error: {error}"));
                                    });
                                }
                            }
                        });
                    });
                    ui_panel.add_space(4.0);
                    ui_panel.horizontal(|ui| {
                        let old_view = self.view;
                        view_tab(ui, &mut self.view, View::Items, "Items");
                        view_tab(ui, &mut self.view, View::Hooks, "Hooks");
                        view_tab(ui, &mut self.view, View::Diff, "Diff");
                        view_tab(ui, &mut self.view, View::Chats, "Chats");
                        if self.view != old_view && self.view == View::Chats {
                            self.rescan_chats();
                        }
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
                            if let Some(path) = action.open.clone() {
                                self.open_and_report(&path);
                            }
                            return;
                        }
                        View::Hooks => {
                            let action = ui::hooks_panel::show(
                                ui_panel,
                                &self.hook_rows,
                                &mut self.hook_filter,
                            );
                            if let Some(path) = action.open.clone() {
                                self.open_and_report(&path);
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
                        let outcome = batch::toggle(&mut self.items, &indices);
                        if let Some(error) = outcome.error {
                            self.status_msg = Some(if outcome.rollback_errors.is_empty() {
                                format!("Rolled back after {error}")
                            } else {
                                format!(
                                    "Rollback incomplete after {error}: {}",
                                    outcome.rollback_errors.join("; ")
                                )
                            });
                        } else {
                            self.status_msg = Some(format!(
                                "{} items {}",
                                outcome.toggled,
                                if want_enabled { "enabled" } else { "disabled" }
                            ));
                        }
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
                            let path = self.items[idx].path.clone();
                            self.editor.open(path).unwrap_or_else(|error| {
                                self.status_msg = Some(format!("Error: {error}"));
                            });
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

        if self.scope != old_scope {
            self.refresh();
            if self.view == View::Chats {
                self.rescan_chats();
            }
        } else if self.selected_provider != old_provider {
            self.rescan_items();
            self.filter = FilterKind::All;
            if self.view == View::Chats {
                self.rescan_chats();
            }
        }
        if self.selected_provider.is_none()
            && matches!(self.view, View::Items | View::Hooks | View::Diff)
        {
            self.view = View::Chats;
        }
    }
}

impl App {
    fn open_and_report(&mut self, path: &std::path::Path) {
        if let Err(error) = open_path(path) {
            self.status_msg = Some(format!("Could not open {}: {error}", path.display()));
        }
    }

    fn show_chats(&mut self, ui_panel: &mut egui::Ui) {
        let provider_label = self
            .selected_provider
            .map(|p| p.label())
            .unwrap_or("All Providers");
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
            let title = match (self.chat_trash_mode, self.selected_provider) {
                (true, Some(provider)) => format!("{} Chats Trash", provider.label()),
                (true, None) => "All Providers Chats Trash".into(),
                (false, _) => format!("{} Chats", provider_label),
            };
            ui.label(
                egui::RichText::new(title)
                    .font(ui::theme::heading_font())
                    .color(ui::theme::TEXT_PRIMARY),
            );
        });
        ui_panel.add_space(4.0);
        let list = if self.chat_trash_mode {
            &self.chat_trash
        } else {
            &self.chat_sessions
        };
        let action = ui::chat_panel::show(
            ui_panel,
            list,
            &mut self.chat_selection,
            &mut self.chat_search,
            self.chat_trash_mode,
            &mut self.chat_delete_confirm,
        );
        if action.toggle_trash {
            self.chat_delete_confirm = None;
        }
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
        if let Some(target) = action.convert_archive_to {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("AgentSwitch chats", &["json", "zip"])
                .set_directory(chat::exports_dir())
                .pick_file()
            {
                match chat::convert_archive_file(&path, target) {
                    Ok((out, skipped)) => {
                        self.status_msg = Some(if skipped == 0 {
                            format!(
                                "Converted archive for {}: {} — use Import to place it in {}",
                                target.label(),
                                out.display(),
                                target.label()
                            )
                        } else {
                            format!(
                                "Converted archive for {}: {} ({skipped} Antigravity chat(s) skipped)",
                                target.label(),
                                out.display()
                            )
                        });
                    }
                    Err(e) => self.status_msg = Some(format!("Convert error: {e}")),
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
        if !action.convert.is_empty() {
            let mut ok = 0usize;
            let mut failed = 0usize;
            let mut errors: Vec<String> = Vec::new();
            for (idx, target) in action.convert {
                let Some(session) = self.chat_sessions.get(idx) else {
                    failed += 1;
                    continue;
                };
                match chat::convert_session(session, target) {
                    Ok(_) => ok += 1,
                    Err(error) => record_failure(&mut errors, &mut failed, error),
                }
            }
            self.rescan_chats();
            self.status_msg = Some(format!(
                "{ok} chat(s) converted{}",
                failure_note(failed, &errors)
            ));
        }
        if !action.trash.is_empty() {
            let (ok, failed, errors) =
                batch_over(&self.chat_sessions, action.trash, chat::soft_delete);
            self.chat_selection.clear();
            self.rescan_chats();
            self.status_msg = Some(format!(
                "{ok} chats moved to Trash{}",
                failure_note(failed, &errors)
            ));
        }
        if !action.restore.is_empty() {
            let mut ok = 0usize;
            let mut failed = 0usize;
            let mut alternates = 0usize;
            let mut errors: Vec<String> = Vec::new();
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
                        Err(error) => record_failure(&mut errors, &mut failed, error),
                    }
                }
            }
            self.chat_selection.clear();
            self.rescan_chats();
            let mut message = format!("{ok} chats restored{}", failure_note(failed, &errors));
            if alternates > 0 {
                message.push_str(&format!(", {alternates} renamed"));
            }
            self.status_msg = Some(message);
        }
        if !action.delete_forever.is_empty() || !action.empty_visible.is_empty() {
            let mut indices: Vec<usize> = Vec::new();
            indices.extend(action.delete_forever);
            indices.extend(action.empty_visible);
            indices.sort_unstable();
            indices.dedup();
            let (ok, failed, errors) =
                batch_over(&self.chat_trash, indices, chat::delete_trash_forever);
            self.chat_selection.clear();
            self.rescan_chats();
            self.status_msg = Some(format!(
                "{ok} trash chats deleted forever{}",
                failure_note(failed, &errors)
            ));
        }
    }
}

fn open_path(path: &std::path::Path) -> std::io::Result<()> {
    let result = if cfg!(windows) {
        std::process::Command::new("explorer").arg(path).spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(path).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(path).spawn()
    };
    result.map(|_| ())
}

fn truncate_error(error: anyhow::Error) -> String {
    let text = error.to_string();
    if text.chars().count() <= 200 {
        text
    } else {
        let mut short: String = text.chars().take(200).collect();
        short.push('…');
        short
    }
}

fn batch_over(
    list: &[ChatSession],
    indices: Vec<usize>,
    act: fn(&ChatSession) -> anyhow::Result<()>,
) -> (usize, usize, Vec<String>) {
    let (mut ok, mut failed, mut errors) = (0usize, 0usize, Vec::new());
    for idx in indices {
        match list.get(idx).map(act) {
            Some(Ok(())) => ok += 1,
            Some(Err(error)) => record_failure(&mut errors, &mut failed, error),
            None => failed += 1,
        }
    }
    (ok, failed, errors)
}

fn record_failure(errors: &mut Vec<String>, failed: &mut usize, error: anyhow::Error) {
    *failed += 1;
    let text = truncate_error(error);
    if errors.len() < 3 && !errors.contains(&text) {
        errors.push(text);
    }
}

fn failure_note(failed: usize, errors: &[String]) -> String {
    if failed == 0 {
        return String::new();
    }
    format!(" — {failed} failed: {}", errors.join("; "))
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
