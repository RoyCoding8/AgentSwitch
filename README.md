# AgentSwitch

AgentSwitch is a fast, native GUI application built in Rust for managing AI coding agent configurations. It allows developers to easily toggle skills, hooks, rules, and configurations across multiple AI coding assistants, avoiding the need to manually move or edit configuration files.

## Features

- **Multi-Provider Support**: Seamlessly integrates with AI coding agents including:
  - Claude Code
  - Codex CLI
  - Gemini CLI/Antigravity 
  - Kiro
  - OpenCode
- **Native GUI**: Built using `egui` for a lightweight, GPU-accelerated, native Windows experience (only ~3.5MB for the release build).
- **Scope Management**: Switch between managing configurations globally (in your User directory) or locally (per Project workspace).
- **One-Click Toggles**: Quickly enable or disable tools, hooks, and MCP servers without manual JSON editing.
- **Inline Editor**: Edit markdown-based instruction files (like `CLAUDE.md`, `GEMINI.md`) directly within the application.
- **Clean Theme**: A professional, high-contrast dark mode tailored for developers.

## Installation

### Prerequisites
You need the [Rust Toolchain](https://rustup.rs/) installed to build from source.

### Build from source
1. Clone the repository:
   ```bash
   git clone https://github.com/yourusername/AgentSwitch.git
   cd AgentSwitch
   ```
2. Build the optimized release executable:
   ```bash
   cargo build --release
   ```
3. The standalone `.exe` will be located at `target/release/agent-switch.exe`. You can move this file anywhere or create a shortcut to it.

## Usage
Simply launch the `agent-switch.exe` file. The application will automatically scan your current directory for AI agent configuration files. You can browse to other workspaces using the folder icon in the sidebar.

## License

This project is licensed under the Apache 2.0 License - see the [LICENSE](LICENSE) file for details.
