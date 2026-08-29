use crate::{
    chat::{self, ChatSession},
    ui::theme,
};
use egui::{Button, RichText, TextEdit, Ui};
use std::collections::HashSet;

#[derive(Default)]
pub struct ChatAction {
    pub export: Vec<usize>,
    pub trash: Vec<usize>,
    pub restore: Vec<usize>,
    pub delete_forever: Vec<usize>,
    pub empty_visible: Vec<usize>,
    pub convert: Vec<(usize, crate::chat::ChatProvider)>,
    pub convert_archive_to: Option<crate::chat::ChatProvider>,
    pub import: bool,
    pub refresh: bool,
    pub toggle_trash: bool,
}

pub fn show(
    ui: &mut Ui,
    sessions: &[ChatSession],
    selected: &mut HashSet<String>,
    search: &mut String,
    trash_mode: bool,
    delete_confirm: &mut Option<(Vec<String>, bool)>,
) -> ChatAction {
    let mut action = ChatAction::default();
    if !trash_mode {
        *delete_confirm = None;
    }
    let mut request_arm: Option<(Vec<String>, bool)> = None;
    let visible = visible_indices(sessions, search);
    let selected_indices = selected_indices(sessions, selected);
    ui.horizontal_wrapped(|ui| {
        if ui.button("Import").clicked() {
            action.import = true;
        }
        ui.menu_button("Convert archive…", |ui| {
            for target in chat::conversion_targets() {
                if ui.button(target.label()).clicked() {
                    ui.close_menu();
                    action.convert_archive_to = Some(target);
                }
            }
        });
        if ui.button("Refresh").clicked() {
            action.refresh = true;
        }
        if ui
            .button(if trash_mode { "Active chats" } else { "Trash" })
            .clicked()
        {
            action.toggle_trash = true;
        }
        ui.separator();
        ui.label(
            RichText::new("Search")
                .font(theme::small_font())
                .color(theme::TEXT_DIM),
        );
        ui.add(TextEdit::singleline(search).desired_width(220.0));
        ui.label(
            RichText::new(format!(
                "{} visible / {} total",
                visible.len(),
                sessions.len()
            ))
            .font(theme::small_font())
            .color(theme::TEXT_DIM),
        );
    });
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        if ui.button("Select all visible").clicked() {
            for idx in &visible {
                selected.insert(chat::session_key(&sessions[*idx]));
            }
        }
        if ui.button("Clear selection").clicked() {
            selected.clear();
        }
        ui.separator();
        if trash_mode {
            let armed = delete_confirm.is_some();
            if ui
                .add_enabled(
                    !selected_indices.is_empty() && !armed,
                    Button::new("Restore selected"),
                )
                .clicked()
            {
                action.restore = selected_indices.clone();
            }
            if ui
                .add_enabled(
                    !selected_indices.is_empty() && !armed,
                    Button::new("Delete selected forever"),
                )
                .clicked()
            {
                request_arm = Some((keys_for(sessions, &selected_indices), false));
            }
            if ui
                .add_enabled(
                    !visible.is_empty() && !armed,
                    Button::new("Empty visible trash"),
                )
                .clicked()
            {
                request_arm = Some((keys_for(sessions, &visible), true));
            }
        } else {
            if ui
                .add_enabled(!selected_indices.is_empty(), Button::new("Export selected"))
                .clicked()
            {
                action.export = selected_indices.clone();
            }
            if ui
                .add_enabled(
                    !selected_indices.is_empty(),
                    Button::new("Move selected to Trash"),
                )
                .clicked()
            {
                action.trash = selected_indices.clone();
            }
            ui.menu_button("Convert selected…", |ui| {
                let providers: Vec<_> = selected_indices
                    .iter()
                    .filter_map(|&i| sessions.get(i))
                    .map(|s| s.provider)
                    .collect();
                for target in chat::conversion_targets() {
                    if !providers.is_empty() && providers.iter().all(|p| *p == target) {
                        continue;
                    }
                    if ui.button(target.label()).clicked() {
                        ui.close_menu();
                        for (&idx, &provider) in selected_indices.iter().zip(providers.iter()) {
                            if provider != target {
                                action.convert.push((idx, target));
                            }
                        }
                    }
                }
            });
        }
        ui.label(
            RichText::new(format!("{} selected", selected_indices.len()))
                .font(theme::small_font())
                .color(theme::TEXT_DIM),
        );
    });
    if let Some(request) = request_arm {
        *delete_confirm = Some(request);
    }
    if let Some((keys, empty_all)) = delete_confirm.clone() {
        let targets = resolve_keys(sessions, &keys);
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(format!(
                    "Permanently delete {} chat(s)? This cannot be undone.",
                    keys.len()
                ))
                .font(theme::small_font())
                .color(theme::YELLOW),
            );
            if ui.button("Yes, delete forever").clicked() {
                if empty_all {
                    action.empty_visible = targets;
                } else {
                    action.delete_forever = targets;
                }
                *delete_confirm = None;
            }
            if ui.button("Cancel").clicked() {
                *delete_confirm = None;
            }
        });
    }
    ui.add_space(8.0);
    if visible.is_empty() {
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new(if trash_mode {
                    "No trash chats found"
                } else {
                    "No local chats found"
                })
                .font(theme::body_font())
                .color(theme::TEXT_DIM),
            );
        });
        return action;
    }
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            let mut i = 0;
            while i < visible.len() {
                let provider = sessions[visible[i]].provider;
                let start = i;
                while i < visible.len() && sessions[visible[i]].provider == provider {
                    i += 1;
                }
                egui::CollapsingHeader::new(
                    RichText::new(format!("{} ({})", provider.label(), i - start))
                        .font(theme::small_font())
                        .color(provider.color()),
                )
                .default_open(true)
                .show(ui, |ui| {
                    let mut j = start;
                    while j < i {
                        let project = sessions[visible[j]].project_path.clone();
                        let project_start = j;
                        while j < i && sessions[visible[j]].project_path == project {
                            j += 1;
                        }
                        egui::CollapsingHeader::new(
                            RichText::new(format!("{project} ({})", j - project_start))
                                .font(theme::small_font())
                                .color(theme::TEXT_DIM),
                        )
                        .default_open(true)
                        .show(ui, |ui| {
                            for &idx in &visible[project_start..j] {
                                row(ui, &sessions[idx], selected, &mut action, idx, !trash_mode);
                                ui.add_space(4.0);
                            }
                        });
                    }
                });
            }
        });
    action
}

fn row(
    ui: &mut Ui,
    session: &ChatSession,
    selected: &mut HashSet<String>,
    action: &mut ChatAction,
    idx: usize,
    allow_convert: bool,
) {
    let key = chat::session_key(session);
    let mut checked = selected.contains(&key);
    theme::card_frame().show(ui, |ui| {
        ui.horizontal(|ui| {
            if ui.checkbox(&mut checked, "").changed() {
                if checked {
                    selected.insert(key.clone());
                } else {
                    selected.remove(&key);
                }
            }
            ui.label(
                RichText::new(&session.title)
                    .font(theme::body_font())
                    .color(theme::TEXT_PRIMARY),
            );
            if session.subagent {
                ui.label(
                    RichText::new("subagent")
                        .font(theme::small_font())
                        .color(theme::TEXT_ACCENT),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(if session.imported {
                        "Imported"
                    } else {
                        "Local"
                    })
                    .font(theme::small_font())
                    .color(if session.imported {
                        theme::TEXT_ACCENT
                    } else {
                        theme::TEXT_DIM
                    }),
                );
            });
        });
        ui.horizontal_wrapped(|ui| {
            if allow_convert && session.provider != crate::chat::ChatProvider::Antigravity {
                ui.menu_button("Convert…", |ui| {
                    for target in chat::convertible_targets(session.provider) {
                        if ui.button(target.label()).clicked() {
                            ui.close_menu();
                            action.convert.push((idx, target));
                        }
                    }
                });
            }
            bit(ui, "updated", &session.updated_at);
            bit(ui, "turns", &session.turn_count.to_string());
            bit(ui, "size", &size_label(session.size_bytes));
            bit(ui, "source", source_label(session));
        });
    });
}

fn visible_indices(sessions: &[ChatSession], search: &str) -> Vec<usize> {
    sessions
        .iter()
        .enumerate()
        .filter(|(_, s)| chat::metadata_matches(s, search))
        .map(|(i, _)| i)
        .collect()
}

fn selected_indices(sessions: &[ChatSession], selected: &HashSet<String>) -> Vec<usize> {
    sessions
        .iter()
        .enumerate()
        .filter(|(_, s)| selected.contains(&chat::session_key(s)))
        .map(|(i, _)| i)
        .collect()
}

fn keys_for(sessions: &[ChatSession], indices: &[usize]) -> Vec<String> {
    indices
        .iter()
        .filter_map(|&i| sessions.get(i))
        .map(chat::session_key)
        .collect()
}

fn resolve_keys(sessions: &[ChatSession], keys: &[String]) -> Vec<usize> {
    let wanted: HashSet<&String> = keys.iter().collect();
    sessions
        .iter()
        .enumerate()
        .filter(|(_, s)| wanted.contains(&chat::session_key(s)))
        .map(|(i, _)| i)
        .collect()
}

fn bit(ui: &mut Ui, key: &str, value: &str) {
    ui.label(
        RichText::new(format!("{key}: {value}"))
            .font(theme::small_font())
            .color(theme::TEXT_DIM),
    );
}

fn source_label(session: &ChatSession) -> &'static str {
    match session.source_kind {
        chat::ChatSourceKind::Jsonl => "JSONL",
        chat::ChatSourceKind::JsonlDir => "JSONL dir",
        chat::ChatSourceKind::ImportedArchive => "archive",
        chat::ChatSourceKind::KiroCli => "kiro-cli",
        chat::ChatSourceKind::OpenCodeDb => "opencode-db",
    }
}

fn size_label(bytes: u64) -> String {
    if bytes > 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / 1024.0 / 1024.0)
    } else if bytes > 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, path: &str) -> ChatSession {
        ChatSession {
            id: id.into(),
            title: id.into(),
            provider: chat::ChatProvider::Kiro,
            project_path: path.into(),
            created_at: None,
            updated_at: String::new(),
            source_path: Some(std::path::PathBuf::from(path)),
            source_kind: chat::ChatSourceKind::KiroCli,
            turn_count: 0,
            size_bytes: 0,
            imported: false,
            subagent: false,
            trash_manifest: None,
        }
    }

    #[test]
    fn resolve_keys_drops_sessions_removed_by_rescan() {
        let a = session("a", "C:\\a.json");
        let b = session("b", "C:\\b.json");
        let c = session("c", "C:\\c.json");
        let before = vec![a.clone(), b.clone(), c.clone()];
        let keys = keys_for(&before, &[0, 2]);

        let after = vec![b.clone(), c.clone()];
        let targets = resolve_keys(&after, &keys);
        assert_eq!(targets, vec![1]);
    }
}
