use crate::types::*;
use crate::ui::theme;
use egui::{CornerRadius, RichText, Sense, Ui, Vec2};

pub fn show(
    ui: &mut Ui,
    providers: &[(ProviderId, bool)],
    selected: &mut Option<ProviderId>,
    scope: &mut Scope,
    workspace: &str,
    on_browse: &mut bool,
) {
    ui.vertical(|ui| {
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("AgentSwitch")
                    .font(theme::heading_font())
                    .color(theme::TEXT_ACCENT),
            );
        });
        ui.add_space(8.0);

        // scope tabs
        ui.horizontal(|ui| {
            for (s, label) in [(Scope::Project, "Project"), (Scope::Global, "Global")] {
                let active = *scope == s;
                let text = RichText::new(label)
                    .font(theme::small_font())
                    .color(if active {
                        theme::TEXT_ACCENT
                    } else {
                        theme::TEXT_DIM
                    });
                if ui.selectable_label(active, text).clicked() {
                    *scope = s;
                }
            }
        });
        ui.add_space(4.0);

        // workspace path
        ui.horizontal(|ui| {
            let path_text = if workspace.len() > 28 {
                format!("...{}", &workspace[workspace.len() - 25..])
            } else {
                workspace.to_string()
            };
            ui.label(
                RichText::new(path_text)
                    .font(theme::small_font())
                    .color(theme::TEXT_DIM),
            );
            if ui
                .small_button("...")
                .on_hover_text("Browse workspace")
                .clicked()
            {
                *on_browse = true;
            }
        });

        ui.add_space(16.0);
        ui.separator();
        ui.add_space(8.0);
        ui.label(
            RichText::new("PROVIDERS")
                .font(theme::small_font())
                .color(theme::TEXT_DIM),
        );
        ui.add_space(4.0);

        for &(id, detected) in providers {
            if !detected {
                continue;
            }
            let is_sel = *selected == Some(id);
            let (r, resp) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.0), Sense::click());
            if resp.hovered() || is_sel {
                ui.painter().rect_filled(
                    r,
                    CornerRadius::same(6),
                    if is_sel {
                        theme::BG_SELECTED
                    } else {
                        theme::BG_HOVER
                    },
                );
            }
            let dot_color = id.color();
            let dot_center = egui::pos2(r.left() + 16.0, r.center().y);
            ui.painter().circle_filled(dot_center, 5.0, dot_color);
            let text_pos = egui::pos2(r.left() + 30.0, r.center().y - 7.0);
            ui.painter().text(
                text_pos,
                egui::Align2::LEFT_TOP,
                id.label(),
                theme::body_font(),
                theme::TEXT_PRIMARY,
            );
            if resp.clicked() {
                *selected = Some(id);
            }
        }
    });
}
