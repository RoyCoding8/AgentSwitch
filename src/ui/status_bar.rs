use crate::process;
use crate::provider;
use crate::types::*;
use crate::ui::theme;
use egui::{RichText, Ui};

pub fn show(ui: &mut Ui, items: &[ConfigItem], providers: &[(ProviderId, bool)]) {
    // Prime the cache in the background; rendering below only reads it so a
    // hanging CLI can never freeze the frame.
    let all_cli_names: Vec<&'static str> = ProviderId::ALL
        .iter()
        .flat_map(|id| provider::cli_names(*id).iter().copied())
        .collect();
    process::warm_up(&all_cli_names);
    ui.horizontal(|ui| {
        let total = items.len();
        let disabled = items
            .iter()
            .filter(|i| i.state == ItemState::Disabled)
            .count();
        let detected: Vec<String> = providers
            .iter()
            .filter(|(_, detected)| *detected)
            .map(|(id, _)| {
                let result = provider::cli_names(*id)
                    .iter()
                    .filter_map(|name| process::shared().probe_cached(name))
                    .find(|result| result.installed);
                match result {
                    Some(result) => result
                        .version
                        .or_else(|| {
                            result
                                .error
                                .map(|error| format!("{} ({error})", id.label()))
                        })
                        .unwrap_or_else(|| id.label().into()),
                    None => format!("{}…", id.label()),
                }
            })
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
