# AgentSwitch

Native desktop GUI for managing AI coding-agent configuration across providers. Toggle skills, hooks, rules, and MCP servers — browse, diff, and manage chat histories — without hand-editing provider files.

## Features

- **Item Toggle** — per-item enable/disable with reversible rename or JSON mutation.
- **Bulk Toggle** — Enable All / Disable All for filtered item categories.
- **Scope Switching** — project-level vs global configuration, with workspace browser.
- **Diff Workbench** — compare project and global configs with stable, secret-safe fingerprints. Detects duplicates, missing targets, and scope conflicts.
- **Hook Cockpit** — static hook inventory showing event, matcher, handler, blocking risk, timeout, duplicates, and project/global overlaps.
- **Chat Manager** — unified chat history browser across Claude Code, Codex CLI, Gemini CLI, Antigravity CLI, and Kiro. Search, export (single JSON or multi-chat ZIP), soft-delete with Trash, and import archived sessions.
- **Inline Editor** — edit instruction files, rules, and steering docs without leaving the app.
- **JSON Backups** — automatic `.bak` creation before any config mutation.
- **Cross-platform** — Windows, Linux, and macOS builds.

## Supported Providers

| Provider | Instruction File | Skills | Hooks | MCP | Other |
|---|---|---|---|---|---|
| Claude Code | `CLAUDE.md` | `.claude/skills/` | `settings.json` | `settings.json` | Rules |
| Codex CLI | `AGENTS.md` | `.codex/skills/`, `.agents/skills/` | `config.toml`, `hooks.json` | `config.toml`, `.mcp.json` | — |
| Gemini CLI | `GEMINI.md`, `AGENTS.md` | `.gemini/skills/` | `settings.json` | `settings.json` | Rules |
| Antigravity CLI | `GEMINI.md`, `AGENTS.md` | `.agents/skills/` | `settings.json` | `mcp_config.json` | — |
| Kiro | — | — | Agent JSON | `settings/mcp.json` | Steering, Specs, Agents |
| OpenCode | `AGENTS.md` | `.opencode/skills/` | Plugins | `opencode.json` | Agents |

## Install

Download the matching binary from [Releases](https://github.com/RoyCoding8/AgentSwitch/releases):

| Platform | Asset |
|---|---|
| Windows x86-64 | `agent-switch-windows-x86_64.exe` |
| Linux x86-64 | `agent-switch-linux-x86_64` |
| macOS Intel | `agent-switch-macos-x86_64` |
| macOS Apple Silicon | `agent-switch-macos-aarch64` |

## Build from Source

Requires the [Rust toolchain](https://rustup.rs/) (1.75+).

```bash
git clone https://github.com/RoyCoding8/AgentSwitch.git
cd AgentSwitch
cargo build --release
```

Output binary:

- **Windows:** `target/release/agent-switch.exe`
- **Linux / macOS:** `target/release/agent-switch`

### Linux Dependencies

```bash
sudo apt-get update
sudo apt-get install -y \
  pkg-config libgtk-3-dev libx11-dev libxi-dev \
  libxkbcommon-dev libwayland-dev libgl1-mesa-dev libasound2-dev
```

## Usage

Launch AgentSwitch from the workspace you want to inspect, or use **Browse** to pick a workspace at runtime.

| Tab | Purpose |
|---|---|
| **Items** | Toggle discovered provider config items (skills, hooks, rules, MCP servers). |
| **Hooks** | Inspect hook execution order, scope, matcher, handler type, blocking risk, duplicates, and project/global overlaps. |
| **Diff** | Compare project vs global config with stable, secret-redacted fingerprints. |
| **Chats** | Browse, search, export, import, and trash chat sessions across all providers. |

> Diff Workbench and Hook Cockpit are read-only diagnostics. Toggle actions remain in **Items**.


## Architecture

```text
src/
  main.rs          eframe entry point
  app.rs           state machine and UI orchestration
  types.rs         shared item, provider, and scope types
  scanner.rs       provider filesystem discovery
  toggler.rs       rename and JSON/TOML mutation logic
  diagnostics.rs   project/global diff workbench engine
  hook_diag.rs     static hook cockpit engine
  chat.rs          chat history scanner, archive, export/import, trash
  editor.rs        inline markdown editor state
  ui/
    mod.rs         module declarations
    theme.rs       dark theme colors, fonts, and style
    sidebar.rs     provider list and scope tabs
    item_list.rs   toggle list with filter tabs
    diff_panel.rs  diff workbench UI
    hooks_panel.rs hook cockpit UI
    chat_panel.rs  chat manager UI
    editor_panel.rs inline editor UI
    status_bar.rs  bottom status summary
```

## License

Apache 2.0. See [LICENSE](LICENSE).
