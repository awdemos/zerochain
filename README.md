<div align="center">

# zerochain

**Build AI agents with `mkdir`.**

Multi-agent orchestration using the filesystem.<br>
Directories are stages. Files are state. Symlinks are data flow.<br>
Optional Lua config engine for self-modifying workflows.

<p>
  <img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License" />
  &nbsp;
  <img src="https://img.shields.io/badge/rust-nightly_1.90+-orange.svg" alt="Rust nightly" />
  &nbsp;
  <img src="https://img.shields.io/badge/unsafe-no-brightgreen.svg" alt="No unsafe" />
  &nbsp;
  <img src="https://img.shields.io/badge/runtime-tokio-9cf.svg" alt="Tokio" />
</p>

**[Quick Start](#quick-start)** · **[Install](#install)** · **[CLI](#cli)** · **[How It Works](#how-workflows-work)** · **[Architecture](#architecture)**

</div>

---

> An implementation of the agent architecture as files and folders proposed by Jake Van Clief
> in a [short YouTube video explainer](https://www.youtube.com/shorts/tbVtt2-qUJo).

### Highlights

| | |
|---|---|
| **Filesystem-native** | No databases needed. Directories are stages, files are state. CLI or HTTP daemon. |
| **Content-addressed** | Blake3 hashing. Every artifact identified by its content hash. |
| **Crash-safe** | Atomic writes, PID-based stale lock detection, automatic recovery. |
| **Deterministic LLM** | Config derived from content hash. Same input, same execution. |
| **Provider-agnostic** | Any OpenAI-compatible API — OpenAI, Ollama, Moonshot, and more. |
| **Zero unsafe** | Pure safe Rust. Async I/O with tokio. Every fallible op returns `Result`. |
| **Self-modifying workflows** | Optional Lua config engine. Stages can insert/remove subsequent stages at runtime. Entirely opt-in — plain `CONTEXT.md` works unchanged. |

## Quick Start

```sh
# Install and configure oh-my-opencode by following the instructions here:
# https://raw.githubusercontent.com/code-yeongyu/oh-my-openagent/refs/heads/dev/docs/guide/installation.md

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
# Kimi K2.5 (Moonshot AI)
export OPENAI_API_KEY="your-moonshot-key"
export ZEROCHAIN_BASE_URL="https://api.moonshot.ai/v1"
export ZEROCHAIN_MODEL="kimi-k2.5"

# OpenAI (default)
export OPENAI_API_KEY="sk-..."
export ZEROCHAIN_MODEL="gpt-4o"

# Ollama (local)
export OPENAI_API_KEY="ollama"
export ZEROCHAIN_BASE_URL="http://localhost:11434/v1"
export ZEROCHAIN_MODEL="llama3"
```

## Provider Profiles

Zerochain supports provider-specific features through **profiles** — opt-in flags in `CONTEXT.md` frontmatter. Without any profile set, behavior is identical to previous versions.

### Quick Example: Kimi K2.5 with Reasoning Capture

```yaml
---
provider_profile: kimi-k2
role: senior code reviewer
thinking_mode: extended
capture_reasoning: true
---
Review the authentication flow for security vulnerabilities.
```

Running this stage produces two files in `output/`:
- `result.md` — the final answer
- `reasoning.md` — the model's chain-of-thought

### Available Flags

| Flag | Default | Description |
|---|---|---|
| `provider_profile` | `generic` | Set to `kimi-k2` to enable Kimi-specific handling |
| `thinking_mode` | `default` | `default`, `disabled`, or `extended` (injects thinking controls into request) |
| `capture_reasoning` | `false` | Writes `reasoning_content` to `output/reasoning.md` |
| `multimodal_input` | `[]` | Array of file attachments (images) sent with the prompt |

### Thinking Modes

```yaml
# Disable thinking (faster, cheaper)
thinking_mode: disabled

# Extended thinking with a token budget
thinking_mode: extended   # defaults to 8192 budget tokens
```

### Multimodal Input

```yaml
---
provider_profile: kimi-k2
multimodal_input:
  - type: image
    path: "./wireframes/auth-flow.png"
    detail: high
---
Describe what you see in the wireframe.
```

### Environment Variable Fallbacks

Set these globally — `CONTEXT.md` frontmatter always takes precedence:

```sh
export ZEROCHAIN_PROVIDER_PROFILE="kimi-k2"
export ZEROCHAIN_CAPTURE_REASONING="true"
export ZEROCHAIN_THINKING_MODE="extended"
```

### Full Kimi K2.5 Setup

```sh
# 1. Set your Moonshot API key
export OPENAI_API_KEY="sk-your-key"

# 2. Point to Kimi API
export ZEROCHAIN_BASE_URL="https://api.moonshot.ai/v1"
export ZEROCHAIN_MODEL="kimi-k2.5"

# 3. Create a workflow with a kimi-k2 profile stage
zerochain init --name my-task
# Edit 00_spec/CONTEXT.md to add provider_profile: kimi-k2

# 4. Run
zerochain run my-task
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

## Container (zerochaind)

Run zerochain as a stateless HTTP daemon inside a container. Every stage completion, error, DAG mutation, and state change is auto-committed via jj for a full audit trail.

### Build

```sh
docker build -t zerochaind .
```

### Run

```sh
docker run -d \
  -p 8080:8080 \
  -e OPENAI_API_KEY="sk-..." \
  -e ZEROCHAIN_BASE_URL="https://api.openai.com/v1" \
  -e ZEROCHAIN_MODEL="gpt-4o" \
  -v zerochain-data:/workspace \
  zerochaind
```

### API

| Method | Endpoint | Description |
|---|---|---|
| `GET` | `/v1/health` | Health check |
| `POST` | `/v1/workflows` | Init workflow (body: `{"name", "template"?}`) |
| `GET` | `/v1/workflows` | List workflows |
| `GET` | `/v1/workflows/{id}` | Workflow status |
| `POST` | `/v1/workflows/{id}/run` | Run next pending stage |
| `POST` | `/v1/workflows/{id}/run/{stage}` | Run specific stage |
| `POST` | `/v1/workflows/{id}/approve/{stage}` | Approve stage |
| `POST` | `/v1/workflows/{id}/reject/{stage}` | Reject stage (body: `{"feedback"?}`) |
| `GET` | `/v1/workflows/{id}/output/{stage}` | Read `result.md` |
| `GET` | `/v1/workflows/{id}/reasoning/{stage}` | Read `reasoning.md` |

### Quick Example

```sh
# Init a workflow
curl -X POST http://localhost:8080/v1/workflows \
  -H 'Content-Type: application/json' \
  -d '{"name": "my-task"}'

# Run the first stage
curl -X POST http://localhost:8080/v1/workflows/my-task/run

# Check status
curl http://localhost:8080/v1/workflows/my-task

# Read output
curl http://localhost:8080/v1/workflows/my-task/output/00_spec
```

### Environment Variables

| Variable | Default | Description |
|---|---|---|
| `ZEROCHAIN_LISTEN` | `0.0.0.0:8080` | Listen address |
| `ZEROCHAIN_WORKSPACE` | `/workspace` | Workspace root inside container |
| `OPENAI_API_KEY` | — | Required for LLM calls |
| `ZEROCHAIN_BASE_URL` | `https://api.openai.com/v1` | API base URL |
| `ZEROCHAIN_MODEL` | `gpt-4o` | Model name |

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

### CONTEXT.lua (Programmable Config)

Replace `CONTEXT.md` with a Lua script for dynamic stage configuration:

```lua
-- CONTEXT.lua
return {
  role = "senior rust developer",
  provider_profile = "kimi-k2",
  thinking_mode = "extended",
  capture_reasoning = true,
  human_gate = false,
}
```

**Lifecycle hooks** — define functions that run at specific points:

```lua
-- CONTEXT.lua
return {
  role = "code reviewer",
  provider_profile = "kimi-k2",
}

function on_validate(ctx)
  -- Skip this stage if env var is set
  if ctx:get_env("SKIP_SLOW_STAGES") == "true" then
    ctx:set_skip(true)
  end
end

function on_complete(ctx)
  local output = ctx:read_output()

  -- Dynamically add a review stage if issues found
  if output and output:match("NEEDS_REVIEW") then
    ctx:insert_stage_after("01b_manual_review")
  end

  -- Store value for later stages
  ctx:store("review_passed", true)
end
```

**Available hooks:**

| Hook | When | Can do |
|---|---|---|
| `on_validate` | Before LLM call | Skip stage, check env vars |
| `on_complete` | After LLM response written | Read output, mutate DAG, store data |

**Available `ctx` methods:**

| Method | Returns | Description |
|---|---|---|
| `ctx:get_env(key)` | `string\|nil` | Read environment variable |
| `ctx:read_output()` | `string\|nil` | Read LLM output (on_complete only) |
| `ctx:token_usage()` | `number\|nil` | Token count used |
| `ctx:set_skip(bool)` | — | Skip this stage |
| `ctx:list_stages()` | `{string}` | List all stage names in workflow |
| `ctx:stage_complete(name)` | `bool` | Check if a stage is complete |
| `ctx:stage_output(name)` | `string\|nil` | Read another stage's output |
| `ctx:insert_stage_after(name)` | — | Add a new stage after current |
| `ctx:remove_stage(name)` | — | Remove a stage from the workflow |
| `ctx:store(key, value)` | — | Save value for other stages |
| `ctx:load(key)` | `any` | Retrieve stored value |

**Shared state** is persisted to `.state/lua_store.json` in the workflow root.

**Sandboxing:** Lua scripts run with no `io`, `os`, `package`, or `debug` libraries. 10MB memory limit, 1M instruction limit.

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
| **zerochain-llm** | LLM provider trait. OpenAI-compatible HTTP backend. Provider profiles (generic, kimi-k2) for per-model feature augmentation. Deterministic config derived from content CID via Blake3 seed. Tool call support. |
| **zerochain-core** | Workflow model. Stages, execution plans, parallel groups, context inheritance, Lua config engine with lifecycle hooks and DAG mutation, Backlog.md task parsing, optional Jujutsu integration. |
| **zerochain-daemon** | CLI binary. Init, run, approve, and inspect workflows from the command line. |
| **zerochain-server** | HTTP daemon (`zerochaind`). REST API wrapping AppState, jj auto-commit for audit trail. Runs in a Wolfi container. |

## Roadmap

- [x] Chainguard container execution for stage isolation
- [ ] Btrfs copy-on-write snapshots (zero-copy stage isolation)
- [ ] OpenCode TypeScript plugin
- [ ] Dagger CI module
- [ ] Template registry for common workflow patterns

---

<div align="center">

Released under the [MIT License](LICENSE).

</div>
