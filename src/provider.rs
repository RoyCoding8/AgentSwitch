use crate::types::{ProviderId, Scope};
use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};

pub fn home_dir() -> Result<PathBuf> {
    if let Some(home) = env_path("AGENT_SWITCH_HOME") {
        return Ok(home);
    }
    dirs::home_dir().ok_or_else(|| anyhow!("cannot determine the user home directory"))
}

pub fn provider_dir(id: ProviderId, root: &Path, scope: Scope) -> Result<PathBuf> {
    if scope == Scope::Project {
        return Ok(project_dir(id, root));
    }
    global_dir(id)
}

pub fn project_dir(id: ProviderId, root: &Path) -> PathBuf {
    match id {
        ProviderId::Claude => root.join(".claude"),
        ProviderId::Codex => root.join(".codex"),
        ProviderId::Antigravity => root.join(".agents"),
        ProviderId::Kiro => root.join(".kiro"),
        ProviderId::OpenCode => root.join(".opencode"),
        ProviderId::Zcode => root.join(".zcode"),
        ProviderId::Junie => root.join(".junie"),
        ProviderId::Muse => root.join(".muse"),
        ProviderId::Grok => root.join(".grok"),
    }
}

pub fn global_dir(id: ProviderId) -> Result<PathBuf> {
    let home = home_dir()?;
    Ok(match id {
        ProviderId::Claude => env_path("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home.join(".claude")),
        ProviderId::Codex => env_path("CODEX_HOME").unwrap_or_else(|| home.join(".codex")),
        ProviderId::Antigravity => env_path("ANTIGRAVITY_HOME")
            .unwrap_or_else(|| home.join(".gemini").join("antigravity-cli")),
        ProviderId::Kiro => env_path("KIRO_HOME").unwrap_or_else(|| home.join(".kiro")),
        ProviderId::OpenCode => {
            env_path("OPENCODE_CONFIG_DIR").unwrap_or_else(|| home.join(".config").join("opencode"))
        }
        ProviderId::Zcode => env_path("ZCODE_HOME").unwrap_or_else(|| home.join(".zcode")),
        ProviderId::Junie => home.join(".junie"),
        ProviderId::Muse => xdg_config_dir().unwrap_or_else(|| home.join(".config")).join("muse"),
        ProviderId::Grok => env_path("GROK_HOME").unwrap_or_else(|| home.join(".grok")),
    })
}

fn xdg_config_dir() -> Option<PathBuf> {
    env_path("XDG_CONFIG_HOME")
}

pub fn instruction_files(id: ProviderId, root: &Path, scope: Scope) -> Result<Vec<PathBuf>> {
    let dir = provider_dir(id, root, scope)?;
    Ok(match (id, scope) {
        (ProviderId::Claude, Scope::Project) => vec![root.join("CLAUDE.md")],
        (ProviderId::Claude, Scope::Global) => vec![dir.join("CLAUDE.md")],
        (ProviderId::Codex, Scope::Project) => vec![root.join("AGENTS.md")],
        (ProviderId::Codex, Scope::Global) => vec![dir.join("AGENTS.md")],
        (ProviderId::Antigravity, Scope::Project) => {
            vec![root.join("GEMINI.md"), root.join("AGENTS.md")]
        }
        (ProviderId::Antigravity, Scope::Global) => {
            vec![dir.join("GEMINI.md"), dir.join("AGENTS.md")]
        }
        (ProviderId::Kiro, Scope::Project) => {
            vec![dir.join("steering").join("instructions.md")]
        }
        (ProviderId::Kiro, Scope::Global) => {
            vec![dir.join("steering").join("instructions.md")]
        }
        (ProviderId::OpenCode, Scope::Project) => vec![root.join("AGENTS.md")],
        (ProviderId::OpenCode, Scope::Global) => vec![dir.join("AGENTS.md")],
        (ProviderId::Zcode, Scope::Project) => vec![root.join("AGENTS.md")],
        (ProviderId::Zcode, Scope::Global) => vec![dir.join("AGENTS.md")],
        (ProviderId::Junie, Scope::Project) => vec![
            dir.join("AGENTS.md"),
            root.join("AGENTS.md"),
            dir.join("playbook.md"),
            dir.join("guidelines.md"),
        ],
        (ProviderId::Junie, Scope::Global) => vec![],
        (ProviderId::Muse, Scope::Project) => {
            vec![root.join("AGENTS.md"), root.join(".agents").join("AGENTS.md")]
        }
        (ProviderId::Muse, Scope::Global) => vec![dir.join("AGENTS.md")],
        (ProviderId::Grok, Scope::Project) => vec![root.join("AGENTS.md")],
        (ProviderId::Grok, Scope::Global) => vec![dir.join("AGENTS.md")],
    })
}

pub fn cli_names(id: ProviderId) -> &'static [&'static str] {
    match id {
        ProviderId::Claude => &["claude"],
        ProviderId::Codex => &["codex"],
        ProviderId::Antigravity => &["agy"],
        ProviderId::Kiro => &["kiro-cli", "kiro"],
        ProviderId::OpenCode => &["opencode"],
        ProviderId::Zcode => &["zcode"],
        ProviderId::Junie => &["junie"],
        ProviderId::Muse => &["muse"],
        ProviderId::Grok => &["grok"],
    }
}

pub fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_paths_do_not_require_a_home_directory() {
        let root = Path::new("workspace");
        assert_eq!(
            provider_dir(ProviderId::Antigravity, root, Scope::Project).unwrap(),
            root.join(".agents")
        );
        assert_eq!(
            instruction_files(ProviderId::Antigravity, root, Scope::Project).unwrap(),
            vec![root.join("GEMINI.md"), root.join("AGENTS.md")]
        );
    }

    #[test]
    fn antigravity_detects_only_the_current_cli_name() {
        assert_eq!(cli_names(ProviderId::Antigravity), &["agy"]);
    }
}
