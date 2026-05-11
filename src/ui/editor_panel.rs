use egui::{Ui, RichText, TextEdit, ScrollArea};
use crate::editor::EditorState;
use crate::ui::theme;

pub fn show(ui: &mut Ui, editor: &mut EditorState) {
    if !editor.is_open() { return; }
    ui.vertical(|ui| {
        // header
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("Editing: {}", editor.filename()))
                .font(theme::heading_font()).color(theme::TEXT_ACCENT));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(RichText::new("Close").color(theme::TEXT_DIM)).clicked() { editor.close(); }
                if editor.dirty {
                    if ui.button(RichText::new("Revert").color(theme::YELLOW)).clicked() { editor.revert(); }
                    if ui.button(RichText::new("Save").color(theme::GREEN)).clicked() { let _ = editor.save(); }
                }
                if editor.dirty {
                    ui.label(RichText::new("modified").font(theme::small_font()).color(theme::YELLOW));
                }
            });
        });
        ui.separator();
        // editor area
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            let resp = ui.add(TextEdit::multiline(&mut editor.content)
                .font(egui::FontId::monospace(13.0))
                .desired_width(f32::INFINITY)
                .desired_rows(30)
                .code_editor());
            if resp.changed() { editor.update_dirty(); }
        });
    });
}
