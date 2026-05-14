use crate::{
    diagnostics::{DiffFilter, DiffRow, DiffSide, DiffStatus},
    types::ItemKind,
    ui::theme,
};
use egui::{CornerRadius, RichText, Ui};
use std::path::PathBuf;

#[derive(Default)]
pub struct DiffAction {
    pub open: Option<PathBuf>,
}

pub fn show(ui: &mut Ui, rows: &[DiffRow], filter: &mut DiffFilter) -> DiffAction {
    let mut action = DiffAction::default();
    filter_tabs(ui, rows, filter);
    ui.add_space(8.0);

    let filtered: Vec<&DiffRow> = rows.iter().filter(|r| filter.matches(r)).collect();
    if filtered.is_empty() {
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new("No matching diff rows")
                    .font(theme::body_font())
                    .color(theme::TEXT_DIM),
            );
        });
        return action;
    }

    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            let mut last_kind: Option<ItemKind> = None;
            for row in filtered {
                if last_kind != Some(row.kind) {
                    if last_kind.is_some() {
                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(4.0);
                    }
                    ui.label(
                        RichText::new(row.kind.label().to_uppercase())
                            .font(theme::small_font())
                            .color(theme::TEXT_DIM),
                    );
                    ui.add_space(4.0);
                    last_kind = Some(row.kind);
                }
                row_card(ui, row, &mut action);
                ui.add_space(4.0);
            }
        });
    action
}

fn filter_tabs(ui: &mut Ui, rows: &[DiffRow], filter: &mut DiffFilter) {
    ui.horizontal_wrapped(|ui| {
        for &f in DiffFilter::ALL {
            let active = *filter == f;
            let count = rows.iter().filter(|r| f.matches(r)).count();
            let text = format!("{} ({count})", f.label());
            if ui
                .selectable_label(
                    active,
                    RichText::new(text)
                        .font(theme::small_font())
                        .color(if active {
                            theme::TEXT_ACCENT
                        } else {
                            theme::TEXT_DIM
                        }),
                )
                .clicked()
            {
                *filter = f;
            }
        }
    });
}

fn row_card(ui: &mut Ui, row: &DiffRow, action: &mut DiffAction) {
    egui::Frame::NONE
        .fill(theme::BG_DARK)
        .corner_radius(CornerRadius::same(4))
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(&row.name)
                        .font(theme::body_font())
                        .color(theme::TEXT_PRIMARY),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(row.status.label())
                            .font(theme::small_font())
                            .color(status_color(row.status)),
                    );
                });
            });
            if !row.warnings.is_empty() {
                ui.add_space(2.0);
                for warning in &row.warnings {
                    ui.label(
                        RichText::new(format!("! {warning}"))
                            .font(theme::small_font())
                            .color(theme::YELLOW),
                    );
                }
            }
            if !row.notes.is_empty() {
                ui.add_space(2.0);
                for note in &row.notes {
                    ui.label(
                        RichText::new(note)
                            .font(theme::small_font())
                            .color(theme::TEXT_DIM),
                    );
                }
            }
            ui.add_space(4.0);
            ui.columns(2, |cols| {
                side(&mut cols[0], "Project", row.project.as_ref(), action);
                side(&mut cols[1], "Global", row.global.as_ref(), action);
            });
        });
}

fn side(ui: &mut Ui, label: &str, side: Option<&DiffSide>, action: &mut DiffAction) {
    ui.vertical(|ui| {
        ui.label(
            RichText::new(label)
                .font(theme::small_font())
                .color(theme::TEXT_ACCENT),
        );
        match side {
            Some(side) => {
                let state = if side.state.is_enabled() { "on" } else { "off" };
                let count = if side.count > 1 {
                    format!(" · {} entries", side.count)
                } else {
                    String::new()
                };
                ui.label(
                    RichText::new(format!("{state}{count}"))
                        .font(theme::small_font())
                        .color(theme::TEXT_PRIMARY),
                );
                ui.label(
                    RichText::new(&side.summary)
                        .font(theme::small_font())
                        .color(theme::TEXT_DIM),
                );
                ui.label(
                    RichText::new(side.path.to_string_lossy())
                        .font(theme::small_font())
                        .color(theme::TEXT_DIM),
                );
                if ui.small_button(format!("Open {label} config")).clicked() {
                    action.open = Some(side.path.clone());
                }
            }
            None => {
                ui.label(
                    RichText::new("-")
                        .font(theme::small_font())
                        .color(theme::TEXT_DIM),
                );
            }
        }
    });
}

fn status_color(status: DiffStatus) -> egui::Color32 {
    match status {
        DiffStatus::Same => theme::GREEN,
        DiffStatus::ProjectOnly | DiffStatus::GlobalOnly => theme::TEXT_ACCENT,
        DiffStatus::Differs => theme::YELLOW,
    }
}
