use crate::types::*;
use crate::ui::theme;
use egui::{CornerRadius, RichText, Sense, Stroke, Ui, Vec2};

pub struct ToggleResult {
    pub index: Option<usize>,
    pub edit: Option<usize>,
}

pub fn show(ui: &mut Ui, items: &[ConfigItem], filter: FilterKind) -> ToggleResult {
    let mut result = ToggleResult {
        index: None,
        edit: None,
    };
    let filtered: Vec<(usize, &ConfigItem)> = items
        .iter()
        .enumerate()
        .filter(|(_, it)| match filter {
            FilterKind::All => true,
            FilterKind::Specific(k) => it.kind == k,
        })
        .collect();

    if filtered.is_empty() {
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new("No items found")
                    .font(theme::body_font())
                    .color(theme::TEXT_DIM),
            );
        });
        return result;
    }

    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            let mut last_kind: Option<ItemKind> = None;
            for (idx, item) in &filtered {
                if last_kind != Some(item.kind) {
                    if last_kind.is_some() {
                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(4.0);
                    }
                    ui.label(
                        RichText::new(item.kind.label().to_uppercase())
                            .font(theme::small_font())
                            .color(theme::TEXT_DIM),
                    );
                    ui.add_space(4.0);
                    last_kind = Some(item.kind);
                }
                let (r, resp) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.0), Sense::click());
                if resp.hovered() {
                    ui.painter()
                        .rect_filled(r, CornerRadius::same(4), theme::BG_HOVER);
                }
                // toggle indicator
                let enabled = item.state.is_enabled();
                let circ_center = egui::pos2(r.left() + 18.0, r.center().y);
                let circ_color = if enabled {
                    theme::GREEN
                } else {
                    egui::Color32::from_rgb(0x44, 0x44, 0x58)
                };
                ui.painter().circle_filled(circ_center, 6.0, circ_color);
                if enabled {
                    ui.painter()
                        .circle_stroke(circ_center, 6.0, Stroke::new(1.0, theme::GREEN));
                }
                // name
                let name_color = if enabled {
                    theme::TEXT_PRIMARY
                } else {
                    theme::TEXT_DIM
                };
                ui.painter().text(
                    egui::pos2(r.left() + 34.0, r.center().y - 7.0),
                    egui::Align2::LEFT_TOP,
                    &item.name,
                    theme::body_font(),
                    name_color,
                );
                // path (right side)
                let path_str = item.path.to_string_lossy();
                let short = if path_str.len() > 35 {
                    format!("...{}", &path_str[path_str.len() - 32..])
                } else {
                    path_str.to_string()
                };
                ui.painter().text(
                    egui::pos2(r.right() - 8.0, r.center().y - 6.0),
                    egui::Align2::RIGHT_TOP,
                    short,
                    theme::small_font(),
                    theme::TEXT_DIM,
                );
                // edit link for editable items
                if item.editable {
                    let edit_rect = egui::Rect::from_center_size(
                        egui::pos2(r.right() - 20.0, r.center().y + 8.0),
                        Vec2::new(30.0, 14.0),
                    );
                    let edit_resp = ui.allocate_rect(edit_rect, Sense::click());
                    let edit_color = if edit_resp.hovered() {
                        theme::TEXT_ACCENT
                    } else {
                        theme::TEXT_DIM
                    };
                    ui.painter().text(
                        edit_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "edit",
                        theme::small_font(),
                        edit_color,
                    );
                    if edit_resp.clicked() {
                        result.edit = Some(*idx);
                    }
                }
                if resp.clicked() {
                    result.index = Some(*idx);
                }
            }
        });
    result
}

pub fn filter_tabs(ui: &mut Ui, current: &mut FilterKind, available_kinds: &[ItemKind]) {
    ui.horizontal(|ui| {
        let all_active = *current == FilterKind::All;
        if ui
            .selectable_label(
                all_active,
                RichText::new("All")
                    .font(theme::small_font())
                    .color(if all_active {
                        theme::TEXT_ACCENT
                    } else {
                        theme::TEXT_DIM
                    }),
            )
            .clicked()
        {
            *current = FilterKind::All;
        }
        for &kind in available_kinds {
            let active = *current == FilterKind::Specific(kind);
            if ui
                .selectable_label(
                    active,
                    RichText::new(kind.label())
                        .font(theme::small_font())
                        .color(if active {
                            theme::TEXT_ACCENT
                        } else {
                            theme::TEXT_DIM
                        }),
                )
                .clicked()
            {
                *current = FilterKind::Specific(kind);
            }
        }
    });
}
