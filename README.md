<div align="center">

# ⛓️ zerochain

### Build AI Agents with `mkdir`

[![Rust](https://img.shields.io/badge/Rust-000000?style=for-the-badge&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg?style=for-the-badge)](https://opensource.org/licenses/MIT)
[![Zero Unsafe](https://img.shields.io/badge/Zero%20Unsafe-✓-success?style=for-the-badge)]()

**Multi-agent orchestration using the filesystem.**  
Directories are stages. Files are state. Symlinks are data flow.

[⚡ Quick Start](#quick-start) · [📖 How It Works](#how-workflows-work) · [🖥️ CLI](#cli) · [🌐 HTTP API](#container-zerochaind) · [🏗️ Architecture](#architecture)

</div>

---

## 🎯 In One Sentence

> Zerochain implements multi-agent AI workflows as files and folders — no databases, no brokers, no network stacks. Just the filesystem, content-addressed storage, and async Rust.

---

## ✨ Highlights

| | |
|---|---|
| **📁 Filesystem-native** | No databases needed. Directories are stages, files are state. CLI or HTTP daemon. |
| **🔒 Content-addressed** | Blake3 hashing. Every artifact identified by its content hash. |
| **💥 Crash-safe** | Atomic writes, PID-based stale lock detection, automatic recovery. |
| **🎯 Deterministic LLM** | Config derived from content hash. Same input, same execution. |
| **🔌 Provider-agnostic** | Any OpenAI-compatible API — OpenAI, Ollama, Moonshot, and more. |
| **🦀 Zero unsafe** | Pure safe Rust. Async I/O with tokio. Every fallible op returns `Result`. |
| **🧬 Self-modifying workflows** | Optional Lua config engine. Stages can insert/remove subsequent stages at runtime. |

---

## ⚡ Quick Start

```bash
# Install (requires Rust nightly 1.90+)
git clone --depth 1 https://github.com/awdemos/zerochain.git
cd zerochain
cargo build --release --workspace

# Configure
export OPENAI_API_KEY="sk-..."

# Create and run a workflow
zerochain init --name my-task
zerochain run my-task
```

That's it. zerochain creates a stage directory, calls the LLM, and writes the result to `output/result.md`.

---

## 🖥️ CLI

```bash
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
```

---

## 🌐 Container (zerochaind)

Run zerochain as a stateless HTTP daemon with full audit trails via jj:

```bash
docker build -t zerochaind .
docker run -d \
  -p 8080:8080 \
  -e OPENAI_API_KEY="sk-..." \
  -v zerochain-data:/workspace \
  zerochaind
```

| Method | Endpoint | Description |
|--------|----------|-------------|
| `POST` | `/v1/workflows` | Initialize workflow |
| `POST` | `/v1/workflows/{id}/run` | Run next pending stage |
| `GET` | `/v1/workflows/{id}` | Workflow status |
| `GET` | `/v1/workflows/{id}/output/{stage}` | Read result |

---

## 🏗️ Architecture

**Content-addressed storage.** All artifacts stored by Blake3 hash. No filenames matter — content identity is the hash.

**Copy-on-write snapshots.** Each stage gets a CoW snapshot of the previous stage's output.

**Deterministic LLM config.** `LLMConfig::deterministic()` derives a Blake3 seed from the content CID for reproducible execution.

### Crate Structure

| Crate | Purpose |
|-------|---------|
| `zerochain-cas` | Blake3 content-addressed storage with atomic writes |
| `zerochain-fs` | Copy-on-write filesystem, advisory locks, stage markers |
| `zerochain-llm` | Provider-agnostic LLM backend with profiles |
| `zerochain-core` | Workflow engine, Lua config, Backlog.md parsing |
| `zerochain-daemon` | CLI binary |
| `zerochain-server` | HTTP daemon (zerochaind) |

---

## 🗺️ Roadmap

- [ ] Chainguard container execution for stage isolation
- [ ] Btrfs copy-on-write snapshots (zero-copy isolation)
- [ ] OpenCode TypeScript plugin
- [ ] Dagger CI module
- [ ] Template registry for common workflow patterns

---

<div align="center">

**© 2026 Andrew White · MIT License**

</div>
