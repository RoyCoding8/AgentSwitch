# AgentSwitch

Native GUI for managing AI coding agent configurations across providers. Toggle skills, hooks, rules, and MCP servers without manual file editing.

## Supported Providers

| Provider | Instruction File | Skills | Hooks | MCP | Other |
|---|---|---|---|---|---|
| Claude Code | `CLAUDE.md` | `.claude/skills/` | `settings.json` | `settings.json` | Rules |
| Codex CLI | `AGENTS.md` | `.codex/skills/` | `hooks.json` | `.mcp.json` | — |
| Gemini CLI | `GEMINI.md` | `.gemini/skills/` | `settings.json` | `settings.json` | Rules |
| Kiro | — | — | Agent JSON | `mcp.json` | Steering, Specs |
| OpenCode | `AGENTS.md` | `.opencode/skills/` | Plugins | `opencode.json` | Agents |

## Features

- **Per-hook toggle** — disable individual hooks without removing them from config
  - Gemini: native `hooks.disabled` array
  - Claude/Kiro/Codex: reversible stash to `_agentswitch_disabled`
- **Project + Global scope** — switch between workspace configs and user-level (`~/`) configs
- **Auto-detection** — only shows providers that are installed
- **Inline editor** — edit `CLAUDE.md`, `GEMINI.md`, rules, steering directly
- **Backup** — creates `.json.bak` before any JSON mutation

## Install

### From release

Download from [Releases](https://github.com/AshishRogannagar/AgentSwitch/releases). Single executable, no dependencies.

### Build from source

```bash
git clone https://github.com/AshishRogannagar/AgentSwitch.git
cd AgentSwitch
cargo build --release
# binary at target/release/agent-switch.exe
```

Requires [Rust toolchain](https://rustup.rs/).

## Usage

Launch `agent-switch.exe`. It scans the current directory for provider configs. Use the sidebar to:
- Switch providers
- Toggle Project/Global scope
- Browse to a different workspace

Click any item to toggle enabled/disabled. Click "edit" on instruction files to open the inline editor.

## Architecture

```
src/
├── main.rs          # eframe entry point
├── app.rs           # state + UI orchestration
├── types.rs         # ConfigItem, HookLoc, enums
├── scanner.rs       # per-provider filesystem discovery
├── toggler.rs       # rename + JSON mutation logic
├── editor.rs        # markdown editor state
└── ui/
    ├── theme.rs     # dark theme constants
    ├── sidebar.rs   # provider list + scope tabs
    ├── item_list.rs # toggleable items
    ├── editor_panel.rs
    └── status_bar.rs
```

## License

Apache 2.0 — see [LICENSE](LICENSE).
