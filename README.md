# zerochain

This is a Multi-agent orchestration tool using text files as Agents and Workflows using the filesystem as a storage system. Everything lives in folders and files. Directories are stages, files are state, symlinks are data flow. It is designed to be as simple
as digitally possibble and can start the agent framework with `mkdir`.

An implementation of the agent architecture as files and folders proposed by Jake Van Clief [in a short youtube video explainer](https://www.youtube.com/shorts/tbVtt2-qUJo).

The main project is Rust based but that is not needed to run this project, you can Run the Bash script of the project if you prefer.

## Quick Start

```sh
# Install
cargo build --release --workspace
cp target/release/zerochain ~/.cargo/bin/

# Set your API key
export OPENAI_API_KEY="sk-..."

# Create and run a workflow
zerochain init --name my-task
zerochain run my-task
```

That's it. zerochain creates a stage directory, calls the LLM, and writes the result to `output/result.md`.

## Install

Requires Rust nightly (1.90+).

Copy this block and run it:

```sh
set -euo pipefail
REPO="https://github.com/awdemos/zerochain.git"
DEST="${ZEROCHAIN_INSTALL_DIR:-$HOME/.local/share/zerochain}"
BIN="${ZEROCHAIN_BIN_DIR:-$HOME/.cargo/bin}"
echo "==> Cloning zerochain..."
git clone --depth 1 "$REPO" "$DEST/src"
cd "$DEST/src"
echo "==> Building (nightly required)..."
cargo build --release --workspace
echo "==> Installing binary..."
cp target/release/zerochain "$BIN/zerochain"
echo "==> Verifying..."
zerochain --help
echo "==> Done. Run 'zerochain init --help' to get started."
```

Or build from a local checkout:

```sh
cargo build --workspace
cargo test --workspace
```

## Configuration

Environment variables, no config files:

| Variable | Default | Description |
|---|---|---|
| `OPENAI_API_KEY` | — | Required for LLM calls |
| `ZEROCHAIN_BASE_URL` | `https://api.openai.com/v1` | API base URL (include version path) |
| `ZEROCHAIN_MODEL` | `glm-5-turbo` | Model name |
| `ZEROCHAIN_WORKSPACE` | `./workspace` | Workspace root |

### Provider Examples

Any OpenAI-compatible API works. Set `ZEROCHAIN_BASE_URL` and `ZEROCHAIN_MODEL`:

```sh
# Z.AI
export OPENAI_API_KEY="your-zai-key"
export ZEROCHAIN_BASE_URL="https://api.z.ai/api/paas/v4"
export ZEROCHAIN_MODEL="glm-5-turbo"

# OpenAI (default)
export OPENAI_API_KEY="sk-..."
export ZEROCHAIN_MODEL="gpt-4o"

# Ollama (local)
export OPENAI_API_KEY="ollama"
export ZEROCHAIN_BASE_URL="http://localhost:11434/v1"
export ZEROCHAIN_MODEL="llama3"
```

## CLI

```sh
# Initialize a workflow from a Backlog.md task
zerochain init --name my-task --path ./backlog.md

# Run the next pending stage
zerochain run my-task

# Run a specific stage
zerochain run my-task --stage 02_design

# Check workflow status
zerochain status my-task

# List all workflows
zerochain list

# Approve a stage waiting for human review
zerochain approve my-task 03_review

# Reject a stage (marks as error)
zerochain reject my-task 03_review --feedback "needs rework"
```

Global `--workspace` flag or `ZEROCHAIN_WORKSPACE` env var sets the workspace root. Defaults to `./workspace`.

## How Workflows Work

A workflow is a directory tree where each subdirectory is a stage:

```
my-workflow/
├── 01_research/
│   ├── input/          # files from previous stage output
│   ├── output/         # stage writes results here
│   ├── CONTEXT.md      # stage instructions + metadata
│   └── .complete       # created when stage finishes
├── 02a_design/         # runs in parallel with 02b
├── 02b_prototype/      # runs in parallel with 02a
└── 03_review/          # runs after both 02a and 02b
```

**Stage naming:** `NN_name` for sequential, `NNa_name`/`NNb_name` for parallel execution within the same group.

**State markers** (mutually exclusive — only one exists at a time):
- `.complete` — stage succeeded
- `.error` — stage failed
- `.executing` — stage is in progress
- `.lock` — advisory lock with PID for concurrent access protection

### CONTEXT.md

Each stage gets a `CONTEXT.md` with YAML frontmatter and a markdown body:

```markdown
---
role: senior rust developer
human_gate: true
---

Review the code in the input directory for correctness, performance,
and adherence to Rust best practices. Output a summary to result.md.
```

**Context inheritance:** Parent stage context is flattened into child stages automatically.

**Human gates:** Set `human_gate: true` to pause execution until approved via `zerochain approve`.

**Crash recovery:** If a process dies mid-execution, the `.executing` marker remains. On restart, stale markers are detected via PID checks and output directories are cleaned before re-execution.

**Concurrency:** Advisory file locks prevent parallel `zerochain run` instances from executing the same stage. Stale locks from dead processes are automatically reclaimed.

## Backlog.md Integration

[Backlog.md](https://backlog.md) is a task management format that lives in your repo as markdown files. zerochain parses Backlog.md tasks and turns them into executable workflow directory structures.

Tasks are defined with YAML frontmatter:

```markdown
---
id: implement-auth
title: Implement authentication
execution:
  stages:
    - research
    - design
    - implement
    - review
acceptance_criteria:
  - Users can log in
  - Sessions persist across restarts
---

Implement JWT-based authentication for the REST API.
```

`zerochain init --path backlog.md` parses this and creates the stage directory structure.

## Architecture

**Content-addressed storage.** All artifacts are stored by Blake3 hash. No filenames matter — content identity is the hash.

**Copy-on-write snapshots.** Each stage execution gets a CoW snapshot of the previous stage's output. `DirectoryCow` does recursive copies today; Btrfs subvolume snapshots are planned for zero-copy.

**Deterministic LLM config.** The `LLMConfig::deterministic()` method derives a Blake3 seed from the content CID, enabling reproducible LLM execution for the same inputs.

**Optional Jujutsu.** If `jj` is installed, every stage completion creates a commit. If not installed, everything works without it.

**No unsafe code.** All I/O is async (tokio). Every fallible operation returns `Result`. Atomic writes use temp file + rename. Advisory file locks protect concurrent access.

## Crates

| Crate | What it does |
|---|---|
| **zerochain-cas** | Blake3 content-addressed storage. Put bytes, get a CID. Two-level directory sharding (`ab/abcdef...`), atomic writes. |
| **zerochain-fs** | Copy-on-write filesystem abstraction. `DirectoryCow` for now, Btrfs later. Atomic file ops, advisory file locking, `.complete`/`.error`/`.executing` stage markers, output cleanup. |
| **zerochain-llm** | LLM provider trait. OpenAI-compatible HTTP backend. Deterministic config derived from content CID via Blake3 seed. Tool call support. |
| **zerochain-core** | Workflow model. Stages, execution plans, parallel groups, context inheritance, Backlog.md task parsing, optional Jujutsu integration. |
| **zerochain-daemon** | CLI binary. Init, run, approve, and inspect workflows from the command line. |

## Roadmap

- [ ] Chainguard container execution for stage isolation
- [ ] Btrfs copy-on-write snapshots (zero-copy stage isolation)
- [ ] OpenCode TypeScript plugin
- [ ] Dagger CI module
- [ ] Template registry for common workflow patterns

## License

MIT
