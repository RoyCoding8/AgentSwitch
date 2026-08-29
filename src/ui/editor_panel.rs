use crate::editor::EditorState;
use crate::ui::theme;
use egui::{RichText, ScrollArea, TextEdit, Ui};

pub fn show(ui: &mut Ui, editor: &mut EditorState) {
    if !editor.is_open() {
        return;
    }
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("Editing: {}", editor.filename()))
                    .font(theme::heading_font())
                    .color(theme::TEXT_ACCENT),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let confirm_close =
                    editor.dirty && editor.error.as_deref() == Some("__confirm_close__");
                if confirm_close {
                    if ui
                        .button(RichText::new("Discard changes").color(theme::YELLOW))
                        .clicked()
                    {
                        editor.close();
                    }
                    if ui.button("Keep editing").clicked() {
                        editor.error = None;
                    }
                } else if ui
                    .button(RichText::new("Close").color(theme::TEXT_DIM))
                    .clicked()
                {
                    if editor.dirty {
                        editor.error = Some("__confirm_close__".into());
                    } else {
                        editor.close();
                    }
                }
                if editor.dirty {
                    if ui
                        .button(RichText::new("Revert").color(theme::YELLOW))
                        .clicked()
                    {
                        editor.revert();
                    }
                    if ui
                        .button(RichText::new("Save").color(theme::GREEN))
                        .clicked()
                    {
                        if let Err(error) = editor.save() {
                            editor.error = Some(error.to_string());
                        }
                    }
                }
                if editor.dirty {
                    ui.label(
                        RichText::new("modified")
                            .font(theme::small_font())
                            .color(theme::YELLOW),
                    );
                }
            });
        });
        match editor.error.as_deref() {
            Some("__confirm_close__") => {
                ui.label(
                    RichText::new("Unsaved changes — close anyway?")
                        .font(theme::small_font())
                        .color(theme::YELLOW),
                );
            }
            Some(message) => {
                ui.label(
                    RichText::new(message)
                        .font(theme::small_font())
                        .color(theme::YELLOW),
                );
            }
            None => {}
        }
        ui.separator();
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            let resp = ui.add(
                TextEdit::multiline(&mut editor.content)
                    .font(egui::FontId::monospace(13.0))
                    .desired_width(f32::INFINITY)
                    .desired_rows(30)
                    .code_editor(),
            );
            if resp.changed() {
                editor.update_dirty();
            }
        });
    });
}
