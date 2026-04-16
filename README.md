# zerochain

This is a multi-agent orchestration system that works with using text files as agents and workflows. Directories are stages, files are state, symlinks are data flow. Your agent framework starts with `mkdir`.

This is an implementation of the agent architecture as files and folders proposed by Jake Van Clief [in a short youtube video explainer](https://www.youtube.com/shorts/tbVtt2-qUJo).

## Configuration

Environment variables, no config files:

| Variable | Default | Description |
|---|---|---|
| `OPENAI_API_KEY` | — | Required for LLM calls |
| `ZEROCHAIN_LLM_PROVIDER` | `openai` | Provider ID |
| `ZEROCHAIN_BASE_URL` | `https://api.openai.com/v1` | API base URL |
| `ZEROCHAIN_MODEL` | `glm5-turbo` | Model name |
| `ZEROCHAIN_WORKSPACE` | `./workspace` | Workspace root |

## Agent Self-Install

Copy this block and run it to build zerochain from source:

```sh
# zerochain — filesystem-native multi-agent workflow engine
# Requires: Rust nightly (rustup default nightly)
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

## Architecture

**Content-addressed storage.** All artifacts are stored by Blake3 hash. No filenames matter — content identity is the hash.

**Copy-on-write snapshots.** Each stage execution gets a CoW snapshot of the previous stage's output. `DirectoryCow` does recursive copies today; Btrfs subvolume snapshots are planned for zero-copy.

**Deterministic LLM config.** The `LLMConfig::deterministic()` method derives a Blake3 seed from the content CID, enabling reproducible LLM execution for the same inputs.

**Optional Jujutsu.** If `jj` is installed, every stage completion creates a commit. If not installed, everything works without it. No hard dependency but the future of zerochain images will be using these.

**No unsafe code.** All I/O is async (tokio). Every fallible operation returns `Result`. Atomic writes use temp file + rename. Advisory file locks protect concurrent access.

## Build

There are two versions: a bash script and a Rust version.
The Rust version is intended to be the production workload
version but the shell version is totally viable.

Requires Rust nightly (1.90+).

```sh
cargo build --workspace
cargo test --workspace
```

115 tests across 4 library crates. The daemon crate is a binary.

## Crates

| Crate | What it does |
|---|---|
| **zerochain-cas** | Blake3 content-addressed storage. Put bytes, get a CID. Two-level directory sharding (`ab/abcdef...`), atomic writes. |
| **zerochain-fs** | Copy-on-write filesystem abstraction. `DirectoryCow` for now, Btrfs later. Atomic file ops, advisory file locking, `.complete`/`.error`/`.executing` stage markers, output cleanup. |
| **zerochain-llm** | LLM provider trait. OpenAI-compatible HTTP backend. Deterministic config derived from content CID via Blake3 seed. Tool call support. |
| **zerochain-core** | Workflow model. Stages, execution plans, parallel groups, context inheritance, Backlog.md task parsing, optional Jujutsu integration. |
| **zerochain-daemon** | CLI binary. Init, run, approve, and inspect workflows from the command line. |

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
│   ├── input/          # symlink to previous stage output
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

**Human gates:** Add `human_gate: true` to the CONTEXT.md YAML frontmatter. Execution pauses until explicitly approved via `zerochain approve`.

**Context inheritance:** Each stage gets a `CONTEXT.md` with YAML frontmatter and markdown body. Parent context is flattened into child stages.

**Crash recovery:** If a process dies mid-execution, the `.executing` marker remains. On restart, stale markers are detected via PID checks. Output directories are cleaned before re-execution.

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

## Configuration

Environment variables, no config files:

| Variable | Default | Description |
|---|---|---|
| `OPENAI_API_KEY` | — | Required for LLM calls |
| `ZEROCHAIN_LLM_PROVIDER` | `openai` | Provider ID |
| `ZEROCHAIN_BASE_URL` | `https://api.openai.com/v1` | API base URL |
| `ZEROCHAIN_MODEL` | `gpt-4o` | Model name |
| `ZEROCHAIN_WORKSPACE` | `./workspace` | Workspace root |

## Agent Self-Install

Copy this block and run it to build zerochain from source:

```sh
# zerochain — filesystem-native multi-agent workflow engine
# Requires: Rust nightly (rustup default nightly)
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

## Architecture

**Content-addressed storage.** All artifacts are stored by Blake3 hash. No filenames matter — content identity is the hash.

**Copy-on-write snapshots.** Each stage execution gets a CoW snapshot of the previous stage's output. `DirectoryCow` does recursive copies today; Btrfs subvolume snapshots are planned for zero-copy.

**Deterministic LLM config.** The `LLMConfig::deterministic()` method derives a Blake3 seed from the content CID, enabling reproducible LLM execution for the same inputs.

**Optional Jujutsu.** If `jj` is installed, every stage completion creates a commit. If not installed, everything works without it. No hard dependency.

**No unsafe code.** All I/O is async (tokio). Every fallible operation returns `Result`. Atomic writes use temp file + rename. Advisory file locks protect concurrent access.

## Roadmap

- [ ] Chainguard container execution for stage isolation
- [ ] Btrfs copy-on-write snapshots (zero-copy stage isolation)
- [ ] OpenCode TypeScript plugin
- [ ] Dagger CI module
- [ ] Template registry for common workflow patterns

## License

MIT
