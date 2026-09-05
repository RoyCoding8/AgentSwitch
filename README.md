# AgentSwitch

Native desktop GUI for managing AI coding-agent configuration across providers. Toggle skills, hooks, rules, and MCP servers — browse, diff, and manage chat histories — without hand-editing provider files.

## Features

- **Item Toggle** — per-item enable/disable with collision-aware moves and provider-specific JSON/TOML mutation.
- **Bulk Toggle** — Enable All / Disable All for filtered item categories with exact file/path rollback on failure.
- **Scope Switching** — project-level vs global configuration, with workspace browser.
- **Diff Workbench** — compare project and global configs with stable, secret-safe fingerprints. Detects duplicates, missing targets, and scope conflicts.
- **Hook Cockpit** — static hook inventory showing event, matcher, handler, blocking risk, timeout, duplicates, and project/global overlaps.
- **Chat Manager** — unified chat history browser across Claude Code, Codex CLI, Kiro, OpenCode, and ZCode. Per-provider filtering when a provider is selected, or browse all providers together. Search, export (single JSON or multi-chat ZIP), soft-delete with Trash — including database-backed OpenCode/ZCode chats, which are archived and then removed from the live SQLite store (this works while the CLI is running) and restored later with their original session identity — plus import of archived sessions, and converting any chat into another harness's native store or exported archive file (Antigravity excluded — its chats are encrypted).
- **Inline Editor** — edit instruction files, rules, and steering docs without leaving the app. Saves are atomic, refuse to clobber external edits, and warn before discarding unsaved changes.
- **Atomic Config Writes** — structured mutations use same-directory atomic replacement, stale-edit detection, and compatible `.bak` files. TOML mutations preserve comments and formatting; JSON mutations preserve key order.
- **Cross-platform** — Windows, Linux, and macOS builds.

> Antigravity (`agy`) is the supported Google CLI. The discontinued Gemini CLI is not treated as a separate provider. Antigravity may still use the documented `GEMINI.md` filename.

## Supported Providers

| Provider | Instruction File | Skills | Hooks | MCP | Native Chats |
|---|---|---|---|---|---|
| Claude Code | `CLAUDE.md` | `.claude/skills/` | `.claude/settings*.json` (stash to sidecar) | Project `.mcp.json`; approval lists in settings | Best-effort JSONL (internal format) |
| Codex CLI | `AGENTS.md` | `.codex/skills/`, `.agents/skills/` | `hooks.json` (stash to sidecar); `config.toml` inline hooks read-only | `config.toml` `mcp_servers` | Best-effort JSONL (internal format) |
| Antigravity CLI (`agy`) | `GEMINI.md`, `AGENTS.md` | `.agents/skills/`, global `skills/` | `.agents/hooks.json` (native per-definition `enabled` flag) | `.agents/mcp_config.json` | Not supported (encrypted/internal) |
| Kiro | Steering documents | Steering, Specs, Agents | `.kiro/hooks/*.json` (native per-hook `enabled` flag); legacy agent-config hooks stashed | `settings/mcp.json` | JSON + JSONL ACP sessions |
| OpenCode | `AGENTS.md` | `.opencode/skills/` plus `.agents/`/`.claude/` compatibility | Plugins | `opencode.json` | SQLite with schema detection |
| ZCode | `AGENTS.md` (workspace) / `~/.zcode/AGENTS.md` (user) | `.zcode/skills/`, `.agents/skills/` | `hooks.events` in `.zcode/config.json` / `~/.zcode/cli/config.json` (native per-entry `enabled` flag) | `mcp.servers` in the same configs (fallback `.agents/mcp.json`) | SQLite (`~/.zcode/cli/db/db.sqlite`) |

<details>
<summary>How hook toggling works per provider</summary>

Hook toggling follows each provider's documented configuration. Claude Code has no per-hook disable setting, and its settings schema rejects unknown keys, so disabled hook entries are moved to a `<config>.agentswitch` sidecar next to the settings file — the settings file itself stays schema-clean — and restored to their original position on re-enable (stashes written by older versions inside `_agentswitch_disabled` keys are still listed and re-enabled). Codex `hooks.json` uses the same sidecar stash; Codex has no per-hook disable flag (only the global `features.hooks` toggle). Antigravity `hooks.json` maps hook names to definitions with a native per-definition `enabled: false` flag, which AgentSwitch toggles directly. Kiro CLI 3.0 hooks live in `.kiro/hooks/*.json` with a native per-hook `enabled` flag; embedded hooks in legacy `agents/*.json` configs use the sidecar stash.

</details>

<details>
<summary>ZCode scope and storage notes</summary>

ZCode support follows z.ai's official configuration guide: user scope lives under `~/.zcode` (override with `ZCODE_HOME`), workspace scope under `<repo>/.zcode`. Hook entries are toggled through ZCode's own documented per-entry `enabled: false` flag; MCP servers are disabled by stashing them out of `mcp.servers`, since that is the key ZCode reads. Chat browsing reads the OpenCode-compatible session/message/part database ZCode ships at `~/.zcode/cli/db/db.sqlite` (override with `ZCODE_DB`).

</details>

<details>
<summary>Chat conversion details</summary>

**Chat conversion** moves sessions between harnesses in two ways. *Direct:* pick a chat and use **Convert…** to write it straight into another installed harness's native store. *File-based migration:* export from harness A to an archive file, run **Convert archive…** on that file (single JSON or multi-chat ZIP), then use **Import** on the converted file and choose the project folder — the chat lands as a first-class session of harness B. This works even after harness A is uninstalled, since conversion operates purely on the exported file. Conversions always re-synthesize target-native events from AgentSwitch's normalized archive; source-harness event lines are never copied across formats, because each harness only parses its own schema. Converted chats are made discoverable by each harness's own mechanism — e.g. Codex sessions get a full native `session_meta` rollout **and a row in Codex's state database** (`state_N.sqlite` `threads`), since `/resume` lists from SQLite rather than scanning disk. Antigravity is never a conversion source or target because its chats are encrypted inside the CLI.

</details>

## Install

Download the matching binary from [Releases](https://github.com/RoyCoding8/AgentSwitch/releases):

| Platform | Asset |
|---|---|
| Windows x86-64 | `agent-switch-windows-x86_64.exe` |
| Linux x86-64 | `agent-switch-linux-x86_64` |
| macOS Intel | `agent-switch-macos-x86_64` |
| macOS Apple Silicon | `agent-switch-macos-aarch64` |

## Build from Source

Requires the [Rust toolchain](https://rustup.rs/) (1.75+). SQLite is bundled via `rusqlite` — no system dependency needed.

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
  batch.rs         exact multi-item recovery and rollback
  config_store.rs  atomic writes, backups, and verified moves
  provider.rs      current provider paths, CLI names, and instructions
  types.rs         shared item, provider, and scope types
  scanner.rs       provider filesystem discovery
  toggler.rs       rename and provider-specific structured mutations
  diagnostics.rs   project/global diff workbench engine
  hook_diag.rs     static hook cockpit engine
  chat.rs          chat history scanner, archive, export/import, trash, OpenCode SQLite
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
