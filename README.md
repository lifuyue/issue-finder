# Patchbay CLI

<p align="center">
  <strong>Patchbay CLI</strong> is local-first handoff prep for developers using coding agents.
</p>

<p align="center">
  <img src="./docs/assets/patchbay-splash.svg" alt="Patchbay CLI workflow" width="88%" />
</p>

Patchbay finds suitable GitHub issues, ranks them with local heuristics, prepares a safe workspace, and writes a structured handoff package for tools such as Codex, Cursor, Claude Code, and Cline. It also exposes a JSON tool contract so coding agents can list, assess, prepare, and read Patchbay context through structured calls.

It stops before the risky parts: Patchbay does not modify target repository source, install dependencies, run validation commands, commit, push, or create pull requests.

<p align="center">
  <img src="./docs/assets/patchbay-terminal.svg" alt="Patchbay CLI terminal preview" width="88%" />
</p>

---

## Quickstart

### Install Patchbay CLI

Install the published crate with Cargo:

```bash
cargo install patchbay-cli
```

The crate is named `patchbay-cli`; the installed command is `patchbay`.

You can also install directly from this repository:

```bash
cargo install --git https://github.com/lifuyue/patchbay-cli
```

Prefer a prebuilt binary? Run the following on macOS or Linux:

```sh
curl -fsSL https://raw.githubusercontent.com/lifuyue/patchbay-cli/main/install.sh | sh
```

Or run the following on Windows:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://raw.githubusercontent.com/lifuyue/patchbay-cli/main/install.ps1 | iex"
```

You can also download the matching archive from the [latest GitHub Release](https://github.com/lifuyue/patchbay-cli/releases/latest):

- macOS Apple Silicon: `patchbay-aarch64-apple-darwin.tar.gz`
- macOS Intel: `patchbay-x86_64-apple-darwin.tar.gz`
- Linux x86_64: `patchbay-x86_64-unknown-linux-gnu.tar.gz`
- Windows x86_64: `patchbay-x86_64-pc-windows-msvc.zip`

Each archive contains a `patchbay` executable. Put it somewhere on your `PATH`.

### Configure GitHub

Patchbay needs Git and a GitHub token with read access:

```bash
export GITHUB_TOKEN="$(gh auth token)"
```

Then check local readiness:

```bash
patchbay doctor
```

### Prepare your first handoff

```bash
patchbay init
patchbay scout --limit 10
patchbay prepare owner/repo#123
patchbay handoff <inbox-id> --print
```

For isolated local runs, keep generated state out of `~/.patchbay`:

```bash
PATCHBAY_HOME=/tmp/patchbay-demo patchbay doctor
```

### Use the JSON tool contract

Patchbay v1 exposes a CLI JSON adapter for agent-facing tool calls:

```bash
patchbay tools list
patchbay tools call patchbay.scout --arguments '{"limit":10}'
patchbay tools call patchbay.assess --arguments '{"issue":"owner/repo#123"}'
patchbay tools call patchbay.prepare --arguments '{"issue":"owner/repo#123"}'
patchbay tools call patchbay.read_context --arguments '{"handoffId":"<inbox-id>","section":"entry"}'
```

`tools call` prints a single JSON object on stdout. The four v1 tools are `patchbay.scout`, `patchbay.assess`, `patchbay.prepare`, and `patchbay.read_context`.

## Docs

- [**Usage guide**](./docs/usage.md)
- [**Agent-safe preparation runtime**](./docs/agent-safe-preparation-runtime.md)
- [**Rust design notes**](./docs/patchbay-cli-rust-design.md)
- [**Workflow design specs**](./docs/superpowers/specs/)
- [**Repository guidance for coding agents**](./AGENTS.md)

## Development

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all
```

## Release

Patchbay `0.1.0` release assets are published by pushing a `v0.1.0` tag. Use this GitHub repository About text:

```text
Local-first handoff prep for developers using coding agents.
```
