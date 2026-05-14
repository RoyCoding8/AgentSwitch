# AgentSwitch

Native GUI for managing AI coding-agent configuration across providers. Toggle skills, hooks, rules, and MCP servers without hand-editing provider files.

## Features

- Per-hook toggle with reversible disable behavior.
- Project/global scope switching.
- Diff Workbench for project vs global config comparison.
- Hook Cockpit for static hook inventory, conflicts, and risk warnings.
- Inline editor for instruction files and rules.
- JSON backups before config mutation.
- Windows, Linux, and macOS builds.

## Supported Providers

| Provider | Instruction File | Skills | Hooks | MCP | Other |
|---|---|---|---|---|---|
| Claude Code | `CLAUDE.md` | `.claude/skills/` | `settings.json` | `settings.json` | Rules |
| Codex CLI | `AGENTS.md` | `.codex/skills/`, `.agents/skills/` | `config.toml`, `hooks.json` | `config.toml`, `.mcp.json` | - |
| Gemini CLI | `GEMINI.md`, `AGENTS.md` | `.gemini/skills/` | `settings.json` | `settings.json` | Rules |
| Kiro | - | - | Agent JSON | `settings/mcp.json` | Steering, Specs |
| OpenCode | `AGENTS.md` | `.opencode/skills/` | Plugins | `opencode.json` | Agents |

## Install

Download the matching binary from [Releases](https://github.com/AshishRogannagar/AgentSwitch/releases):

- `agent-switch-windows-x86_64.exe`
- `agent-switch-linux-x86_64`
- `agent-switch-macos-x86_64`
- `agent-switch-macos-aarch64`

## Build

Requires the [Rust toolchain](https://rustup.rs/).

```bash
git clone https://github.com/AshishRogannagar/AgentSwitch.git
cd AgentSwitch
cargo build --release
```

Output:

- Windows: `target/release/agent-switch.exe`
- Linux/macOS: `target/release/agent-switch`

Linux builds may need native GUI dependencies:

```bash
sudo apt-get update
sudo apt-get install -y pkg-config libgtk-3-dev libx11-dev libxi-dev libxkbcommon-dev libwayland-dev libgl1-mesa-dev libasound2-dev
```

## Usage

Launch AgentSwitch from the workspace you want to inspect, or use Browse to pick a workspace.

- `Items`: toggle discovered provider config items.
- `Hooks`: inspect hook order, scope, matcher, handler, blocking risk, duplicates, and project/global overlaps.
- `Diff`: compare project and global config with stable, secret-safe fingerprints.

Diff Workbench and Hook Cockpit are read-only. Toggle actions remain in `Items`.

## Releases

CI runs on Windows, Linux, and macOS for pushes, pull requests, and manual dispatch.

Manual release workflow:

1. Open GitHub Actions.
2. Run `Release`.
3. Enter the version without `v`, for example `1.0.0`.
4. The workflow builds release binaries for Windows, Linux, macOS Intel, and macOS Apple Silicon.
5. The workflow publishes all artifacts plus `SHA256SUMS.txt` to GitHub Releases.

## Architecture

```text
src/
  main.rs          eframe entry point
  app.rs           state and UI orchestration
  types.rs         shared item/provider types
  scanner.rs       provider filesystem discovery
  toggler.rs       rename and JSON mutation logic
  diagnostics.rs   project/global diff workbench logic
  hook_diag.rs     static hook cockpit logic
  editor.rs        markdown editor state
  ui/              egui panels and theme
```

## License

Apache 2.0. See [LICENSE](LICENSE).
