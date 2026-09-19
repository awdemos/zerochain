# AGENTS.md — zerochain

Agent session entry point for the zerochain multi-agent filesystem orchestration framework.

## Start here

1. Read `README.md` for the "directories are stages, files are state" mental model and quick-start.
2. Review `Cargo.toml` workspace members and `CONTRIBUTING.md` for contribution conventions.
3. Inspect `dagger.json`/`Makefile` if you need to run the Dagger CI pipeline.
4. Do not commit API keys (`OPENAI_API_KEY`, `MOONSHOT_API_KEY`, etc.) — load them from environment or `.env` files ignored by Git.

## Project layout

```
Cargo.toml                         Workspace manifest
Makefile                           Dagger-based local CI shortcuts
dagger.json                        Dagger module metadata
crates/
  zerochain-core/                  Core workflow/stage abstractions
  zerochain-engine/                Orchestration engine
  zerochain-daemon/                HTTP daemon (axum)
  zerochain-server/                Server crate
  zerochain-llm/                   LLM provider clients (OpenAI-compatible)
  zerochain-memory/                Embedding + vector memory (fastembed)
  zerochain-fs/                    Filesystem primitives: atomic writes, COW, locks
  zerochain-cas/                   Content-addressed storage (blake3)
  zerochain-broker/                Message broker abstractions
  zerochain-error/                 Shared error types
  zerochain-tools/                 CLI binary and utilities
container/                         Container image build scripts
k8s/                               Kubernetes manifests
docs/                              Documentation assets
templates/                         Workflow templates
plugins/                           Runtime plugin examples
```

## Development commands

```bash
# Build the whole workspace
cargo build --workspace

# Run the test suite
cargo test --workspace

# Lint
cargo clippy --workspace --all-targets

# Format check
cargo fmt --check

# Dagger local CI (requires Dagger CLI)
make ci        # lint + test + build
make test      # dagger call test
make build     # dagger call build
make docker    # produce zerochaind-image.tar
```

## Key conventions

- Rust edition 2021, workspace MSRV roughly nightly 1.90+ (check `rust-toolchain.toml` if present).
- No `unsafe` code in the workspace — this is a project goal.
- Fallible ops return `Result`; every crate uses `thiserror` for its error enum.
- Content addressing uses `blake3` hashes.
- Async runtime is `tokio`; HTTP server uses `axum`.
- Logging uses `tracing` with `tracing-subscriber` (env-filter + JSON support).
- Lua config engine is optional; stages can modify the workflow graph at runtime.

## Gotchas

- The repo may contain a `.cargo/config.toml` that points to a non-existent `/.cargo-targets` directory. If local builds fail with "Read-only file system", remove or override the `[build] target-dir` setting.
- Some `zerochain-fs` tests (`cow::tests::is_btrfs_filesystem_returns_false_on_tempdir`, `detect_backend_returns_directory_on_non_btrfs`) assume the development filesystem is **not** btrfs and can fail on btrfs hosts. This is a known environmental sensitivity, not a code bug.
- The daemon needs an OpenAI-compatible API key exported before running.
- Container/K8s manifests reference `zerochaind` image built via `make docker`.

## Quality gates

Run before committing:

```bash
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt --check
```

For full CI parity locally (requires Dagger):

```bash
make ci
```
