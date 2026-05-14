use crate::types::*;
use crate::ui::theme;
use egui::{RichText, Ui};

pub fn show(ui: &mut Ui, items: &[ConfigItem], providers: &[(ProviderId, bool)]) {
    ui.horizontal(|ui| {
        let total = items.len();
        let disabled = items
            .iter()
            .filter(|i| i.state == ItemState::Disabled)
            .count();
        let detected: Vec<&str> = providers
            .iter()
            .filter(|(_, d)| *d)
            .map(|(id, _)| id.label())
            .collect();
        ui.label(
            RichText::new(format!(
                "{} items | {} disabled | {}",
                total,
                disabled,
                detected.join(", ")
            ))
            .font(theme::small_font())
            .color(theme::TEXT_DIM),
        );
    });
}
