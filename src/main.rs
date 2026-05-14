#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod app;
mod diagnostics;
mod editor;
mod hook_diag;
mod scanner;
mod toggler;
mod types;
mod ui;

fn main() -> eframe::Result {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([960.0, 640.0])
            .with_min_inner_size([700.0, 400.0])
            .with_title("AgentSwitch"),
        ..Default::default()
    };
    eframe::run_native(
        "AgentSwitch",
        opts,
        Box::new(|_cc| Ok(Box::new(app::App::new()))),
    )
}
