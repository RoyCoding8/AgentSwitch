use std::path::PathBuf;
use std::collections::HashSet;
use crate::types::*;
use crate::scanner;
use crate::toggler;
use crate::editor::EditorState;
use crate::ui;

pub struct App {
    workspace: PathBuf,
    scope: Scope,
    providers: Vec<(ProviderId, bool)>,
    selected_provider: Option<ProviderId>,
    items: Vec<ConfigItem>,
    filter: FilterKind,
    editor: EditorState,
    status_msg: Option<String>,
    browse_requested: bool,
    first_frame: bool,
}

impl App {
    pub fn new() -> Self {
        let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let mut app = Self {
            workspace, scope: Scope::Project, providers: vec![],
            selected_provider: None, items: vec![], filter: FilterKind::All,
            editor: EditorState::default(), status_msg: None, browse_requested: false,
            first_frame: true,
        };
        app.refresh();
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
        self.providers = ProviderId::ALL.iter()
            .map(|&id| (id, scanner::provider_exists(id, &root, self.scope)))
            .collect();
        // auto-select first detected
        if self.selected_provider.is_none() || !self.providers.iter().any(|(id, d)| *d && Some(*id) == self.selected_provider) {
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
    }

    fn available_kinds(&self) -> Vec<ItemKind> {
        let mut seen = HashSet::new();
        self.items.iter().filter_map(|i| if seen.insert(i.kind) { Some(i.kind) } else { None }).collect()
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.first_frame { ui::theme::apply(ctx); self.first_frame = false; }

        // browse dialog
        if self.browse_requested {
            self.browse_requested = false;
            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                self.workspace = path;
                self.refresh();
            }
        }

        let old_scope = self.scope;
        let old_provider = self.selected_provider;

        // sidebar
        egui::SidePanel::left("sidebar").min_width(170.0).max_width(200.0)
            .frame(egui::Frame::NONE.fill(ui::theme::BG_SIDEBAR).inner_margin(egui::Margin::same(8)))
            .show(ctx, |ui_panel| {
                ui::sidebar::show(
                    ui_panel, &self.providers, &mut self.selected_provider,
                    &mut self.scope, &self.workspace.to_string_lossy(),
                    &mut self.browse_requested,
                );
            });

        // bottom status
        egui::TopBottomPanel::bottom("status").frame(
            egui::Frame::NONE.fill(ui::theme::BG_SIDEBAR).inner_margin(egui::Margin::same(6))
        ).show(ctx, |ui_panel| {
            ui::status_bar::show(ui_panel, &self.items, &self.providers);
            if let Some(msg) = &self.status_msg {
                ui_panel.label(egui::RichText::new(msg).font(ui::theme::small_font()).color(ui::theme::YELLOW));
            }
        });

        // main content
        egui::CentralPanel::default().frame(
            egui::Frame::NONE.fill(ui::theme::BG_PANEL).inner_margin(egui::Margin::same(16))
        ).show(ctx, |ui_panel| {
            if self.editor.is_open() {
                ui::editor_panel::show(ui_panel, &mut self.editor);
            } else if self.selected_provider.is_some() {
                let kinds = self.available_kinds();
                ui::item_list::filter_tabs(ui_panel, &mut self.filter, &kinds);
                ui_panel.add_space(8.0);
                let result = ui::item_list::show(ui_panel, &self.items, self.filter);
                // handle toggle
                if let Some(idx) = result.index {
                    if idx < self.items.len() {
                        match toggler::toggle_item(&mut self.items[idx]) {
                            Ok(()) => self.status_msg = Some(format!("Toggled: {}", self.items[idx].name)),
                            Err(e) => self.status_msg = Some(format!("Error: {e}")),
                        }
                    }
                }
                // handle edit
                if let Some(idx) = result.edit {
                    if idx < self.items.len() && self.items[idx].editable {
                        self.editor.open(self.items[idx].path.clone());
                    }
                }
            } else {
                ui_panel.add_space(60.0);
                ui_panel.vertical_centered(|ui| {
                    ui.label(egui::RichText::new("No providers detected").font(ui::theme::heading_font()).color(ui::theme::TEXT_DIM));
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new("Select a workspace with AI agent config directories").font(ui::theme::body_font()).color(ui::theme::TEXT_DIM));
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
    }
}
