#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod app;
mod batch;
mod chat;
mod config_store;
mod diagnostics;
mod editor;
mod hook_diag;
mod process;
mod provider;
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

#[cfg(test)]
pub(crate) mod test_env {
    use std::path::{Path, PathBuf};

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    pub(crate) fn temp_dir(tag: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("agentswitch-{tag}-{nonce}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    pub(crate) fn temp_file(tag: &str, file: &str, content: &[u8]) -> PathBuf {
        let path = temp_dir(tag).join(file);
        std::fs::write(&path, content).unwrap();
        path
    }

    pub(crate) fn with_env_vars<T>(vars: &[(&str, &Path)], run: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK.lock().unwrap();
        let previous: Vec<(String, Option<std::ffi::OsString>)> = vars
            .iter()
            .map(|(name, _)| ((*name).to_string(), std::env::var_os(name)))
            .collect();
        for (name, value) in vars {
            std::env::set_var(name, value);
        }
        let result = run();
        for ((name, _), (_, previous)) in vars.iter().zip(previous.iter()).rev() {
            match previous {
                Some(previous) => std::env::set_var(name, previous),
                None => std::env::remove_var(name),
            }
        }
        result
    }
}
