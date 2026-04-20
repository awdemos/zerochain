# Contributing to ZeroChain

ZeroChain is developed with [jj](https://github.com/martinvonz/jj), a Git-compatible version control system that records every operation in an immutable log. You can contribute with plain Git if you prefer, but we recommend trying jj — it's the fastest way to understand why ZeroChain's filesystem-native audit trails matter.

## Prerequisites

- [Rust](https://rustup.rs/) nightly 1.90+
- [jj](https://github.com/martinvonz/jj) (`cargo install jj-cli`)

## Getting Started

```bash
# Clone with jj (recommended)
jj git clone https://github.com/awdemos/zerochain.git
cd zerochain

# Or clone with git and add jj later
git clone https://github.com/awdemos/zerochain.git
cd zerochain
jj git init --colocate
```

## Daily Workflow

### Making Changes

```bash
# Edit files as usual, then see what's changed
jj status
jj diff

# Describe the current change (like git commit, but the working copy *is* the commit)
jj describe -m "fix: handle empty stage output"

# Start a new change on top
jj new
```

### Undo and Recovery

```bash
# Oops? Undo the last operation
jj undo

# See the full operation history
jj op log

# Restore to any previous state
jj op restore <operation-id>
```

### Syncing with GitHub

```bash
# Pull latest changes
jj git fetch

# Push your change to a Git branch
jj git push --bookmark my-feature

# Or push to the current branch
jj git push
```

## Why jj?

ZeroChain's architecture is built on the idea that **filesystem state is truth**. jj applies the same principle to source control:

- Every operation is recorded (`jj op log`)
- The working copy is always a commit — no staging area friction
- Branches are mutable bookmarks on an immutable commit graph
- `jj undo` works for *any* operation, not just commits

Using jj to develop ZeroChain means we eat our own cooking. When you see `jj op log` on this codebase, you're seeing the same pattern that powers ZeroChain's workflow audit trails.

## Git Fallback

If you prefer plain Git, that's fine — this is a normal Git repo under the hood. All standard Git commands work. Just be aware that the maintainers use jj locally, so commit messages and branch names may reflect jj's conventions.
