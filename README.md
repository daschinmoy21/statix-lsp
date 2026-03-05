# statix-lsp

A native LSP server for [Statix] — a linter for the Nix programming language. Built with [tower-lsp](https://github.com/ebkalderon/tower-lsp).

Instead of shelling out to the `statix` CLI, this server links directly against the statix lint library and runs lints in-process, giving you instant diagnostics and quick fixes.

## Features

- **Real-time diagnostics** — lint warnings + parse errors on every keystroke
- **Quick fixes** — code actions to auto-apply Statix suggestions
- **Zero subprocess overhead** — native integration, no CLI shelling

## Prerequisites

- [Nix](https://nixos.org/) with flakes enabled
- The [statix] repo cloned locally inside this project:
  ```sh
  git clone https://github.com/molybdenumsoftware/statix.git 
  ```
- VS Code for testing Neovim soon :)  

## Setup

```sh
# Enter the dev shell (provides Rust toolchain, Node.js, etc.)
nix develop

# Build the LSP binary
cargo build

# Install VS Code extension dependencies
npm install
```

## Usage (VS Code)

**Option A** — Launch from the repo:

```sh
code --extensionDevelopmentPath=/path/to/statix-lsp
```

**Option B** — Press `F5` in VS Code with this folder open to launch an Extension Development Host.

Then open any `.nix` file — diagnostics and quick fixes should appear automatically.

### Configuration

| Setting | Default | Description |
|---|---|---|
| `statix-lsp.serverPath` | `""` | Absolute path to a custom `statix-lsp` binary. Leave empty to use `target/debug/statix-lsp`. |

## Project Structure

```
├── src/main.rs        # LSP server (tower-lsp + statix lint integration)
├── extension.js       # VS Code extension entry point
├── package.json       # VS Code extension manifest
├── statix/            # Cloned statix repo (path dependency)
├── flake.nix          # Nix dev shell
└── Cargo.toml         # Rust deps (uses statix/lib as path dep)
```

## How It Works

1. On `didOpen`/`didChange`, the full document text is parsed via `rnix::Root::parse`
2. Parse errors are converted to LSP diagnostics immediately
3. The AST is walked node-by-node, matching `SyntaxKind` against a prebuilt lint map
4. Each matching lint's `validate()` produces `Report`s with optional `Suggestion`s
5. Suggestions are serialized into the diagnostic's `data` field
6. On `codeAction`, the suggestions are deserialized back into `WorkspaceEdit` quick fixes

## License

MIT
