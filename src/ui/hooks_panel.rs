use crate::{
    hook_diag::{HookFilter, HookRow},
    ui::theme,
};
use egui::{CornerRadius, RichText, Ui};
use std::path::PathBuf;

#[derive(Default)]
pub struct HookAction {
    pub open: Option<PathBuf>,
}

pub fn show(ui: &mut Ui, rows: &[HookRow], filter: &mut HookFilter) -> HookAction {
    let mut action = HookAction::default();
    tabs(ui, rows, filter);
    ui.add_space(8.0);
    let rows: Vec<_> = rows.iter().filter(|r| filter.matches(r)).collect();
    if rows.is_empty() {
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new("No matching hooks")
                    .font(theme::body_font())
                    .color(theme::TEXT_DIM),
            );
        });
        return action;
    }
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            let mut last = "";
            for r in rows {
                if last != r.event {
                    if !last.is_empty() {
                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(4.0);
                    }
                    ui.label(
                        RichText::new(r.event.to_uppercase())
                            .font(theme::small_font())
                            .color(theme::TEXT_DIM),
                    );
                    last = &r.event;
                }
                card(ui, r, &mut action);
                ui.add_space(4.0);
            }
        });
    action
}

fn tabs(ui: &mut Ui, rows: &[HookRow], filter: &mut HookFilter) {
    ui.horizontal_wrapped(|ui| {
        for &f in HookFilter::ALL {
            let active = *filter == f;
            let text = format!(
                "{} ({})",
                f.label(),
                rows.iter().filter(|r| f.matches(r)).count()
            );
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

fn card(ui: &mut Ui, r: &HookRow, action: &mut HookAction) {
    egui::Frame::NONE
        .fill(theme::BG_DARK)
        .corner_radius(CornerRadius::same(4))
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(&r.handler)
                        .font(theme::body_font())
                        .color(theme::TEXT_PRIMARY),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let state = if r.state.is_enabled() {
                        "Enabled"
                    } else {
                        "Disabled"
                    };
                    ui.label(
                        RichText::new(format!("{} · #{}", r.scope_label(), r.order))
                            .font(theme::small_font())
                            .color(theme::TEXT_ACCENT),
                    );
                    ui.label(RichText::new(state).font(theme::small_font()).color(
                        if r.state.is_enabled() {
                            theme::GREEN
                        } else {
                            theme::TEXT_DIM
                        },
                    ));
                });
            });
            ui.add_space(2.0);
            ui.horizontal_wrapped(|ui| {
                bit(ui, "matcher", &r.matcher);
                bit(ui, "handler", &r.handler_kind);
                bit(ui, "behavior", &r.behavior);
                bit(ui, "exec", &r.execution);
                bit(
                    ui,
                    "timeout",
                    &r.timeout
                        .map(|t| format!("{t}s"))
                        .unwrap_or_else(|| "-".into()),
                );
            });
            for w in &r.warnings {
                ui.label(
                    RichText::new(format!("! {w}"))
                        .font(theme::small_font())
                        .color(theme::YELLOW),
                );
            }
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(r.path.to_string_lossy())
                        .font(theme::small_font())
                        .color(theme::TEXT_DIM),
                );
                if ui.small_button("Open config").clicked() {
                    action.open = Some(r.path.clone());
                }
            });
        });
}

fn bit(ui: &mut Ui, k: &str, v: &str) {
    ui.label(
        RichText::new(format!("{k}: {v}"))
            .font(theme::small_font())
            .color(theme::TEXT_DIM),
    );
}
