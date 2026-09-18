# Collective Contribution Graph for zerochain

**Date:** 2026-09-18
**Status:** Approved design, awaiting implementation plan
**Inspiration:** *Agora: Git as Shared Memory for Collective AutoResearch* (Zhang et al., NVIDIA, 2026)

## 1. Problem

zerochain workflows are isolated: each `zerochain init` starts from a template with zero
memory of past runs. The only shared substrate is CAS blobs and, for embeddings, a
per-workflow `memory.jsonl` (`crates/zerochain-memory/src/store.rs`) whose chunks carry no
type, no actor, no timestamp, no lineage — nothing that lets one run build on another.
Agora's core finding is that an append-only DAG of typed contributions (result / insight /
hypothesis / verification / report), stored immutably and shared across sessions, converts
duplicated search into cumulative progress.

zerochain's gaps relative to that mechanism:

- No typed contributions — memory chunks are untyped `(text, metadata)` pairs.
- No lineage — no parent edges, no workflow-to-workflow references of any kind.
- No durable cross-run state — every workflow starts from scratch.
- No verification/evidence concept — quality is similarity-only.
- No frontier or diversity views — nothing exposes neglected branches.

## 2. Design decisions (agreed in brainstorming)

| Question | Decision |
|---|---|
| Centerpiece | Cross-run contribution DAG: typed, append-only, parented records shared workspace-wide |
| Storage | Files-first, jj-audited: canonical records as markdown files under the workspace; the jj auto-commit trail zerochain already writes provides immutability/audit. No database. |
| Production | Auto-capture (`setup`/`result` nodes from workflow init and stage completion) **plus** explicit agent tools for `insight`/`hypothesis`/`verification`/`report` |
| Approach | Evolve `zerochain-memory` into a two-layer store (canonical graph + derived index/embeddings) rather than a new crate |

## 3. Data model — the contribution record

One markdown file per contribution, OKF-style YAML frontmatter extending conventions in
`crates/zerochain-core/src/okf.rs`:

```markdown
---
id: c-9f3a21c7d4e8b601          # blake3 of canonical record content, hex
type: result                     # setup | result | insight | hypothesis | verification | report
parents: [c-1a2b3c...]           # contribution IDs this builds on; [] for roots
actor: zerochain/0.2.0           # OKF actor string; human:<id> for CLI humans
created: 2026-09-18T14:03:11Z
workflow: my-task                # provenance, auto-captured nodes only
stage: 03_eval                   # provenance, auto-captured nodes only
metric: {name: bpb, value: 1.899, direction: lower}   # optional
tags: [multi-donor]              # free-form; reserved behavior tags are a later phase
artifacts: [b3:<cid>]            # optional CAS content IDs
verdict: confirmed               # verification type only; requires target: <id>
target: c-abc123...              # verification type only
---
<markdown body>
```

Field rules:

- **Append-only, immutable.** Record files are never mutated after publish.
- **IDs are content hashes.** `id = "c-" + blake3_hex(canonical_serialization(record_without_id))`
  with canonical = JSON, sorted keys. Publishing the identical record twice is a natural
  no-op, giving retry idempotency. The `created` timestamp is part of the hashed record, so
  distinct publishes always produce distinct IDs.
- **Replaceable verdicts are index-computed, never written back** (Agora §3.2): for each
  `(target, actor)` pair the newest `verification` record by `created` is the effective
  verdict; earlier records remain in the DAG. The index applies this at query time. No
  `status` field exists on records.
- **Reserved types:** `setup` (workflow brief root), `result` (experimental outcome,
  success or failure), `insight` (interpretation/synthesis, parents = evidence), `hypothesis`
  (untested proposal; must not carry a metric as if tested — validation rejects
  `type: hypothesis` with a `metric` field), `verification` (requires `target` + `verdict`;
  target must exist, must not itself be a verification), `report` (synthesis across nodes).
- **Negative results** are ordinary records tagged `negative` — no reserved-type machinery.

## 4. Storage: canonical files + derived index (zerochain-memory)

### 4.1 On-disk layout

```
{workspace}/.zerochain/graph/
  contributions/<id>.md     # canonical; append-only; one file per record
  index/graph.jsonl         # derived; safe to delete; rebuilt by scanning contributions/
  index/embeddings.jsonl    # derived; embedding cache over record bodies
```

### 4.2 Components (new, in `zerochain-memory`)

- **`ContributionStore`** — canonical store. `publish(record) -> id`; atomic write
  (`<id>.md.tmp` + rename, matching `zerochain-fs` atomic conventions); duplicate publish
  of identical content is a success no-op; collision of ID with different content returns an
  error (defensive; 64-bit truncated blake3 plus a timestamp in the record makes this
  practically impossible).
- **`GraphIndex`** — derived, rebuildable. Nodes, parent/child edge maps, per-type views.
  Rebuilt by scanning `contributions/` on open; in-memory thereafter; persisted
  `index/graph.jsonl` is a startup-speed cache only and is always regenerable.
  Views: `recent`, `leaves` (records with no children), `open_hypotheses`, `unverified`
  (results with no non-failed verification from any actor), `negative`, `leaders`
  (grouped by metric `name`+`direction`, best first).
  Supersession: effective verdict per `(target, actor)` = newest by `created`, applied
  when computing verification status.
- **Derived embeddings** — embedding index over record bodies using the existing
  `FastEmbedModel` + `chunk_text`; persisted cache at `index/embeddings.jsonl`;
  rebuildable. Powers semantic `graph_query`.

### 4.3 Legacy per-workflow memory (unchanged this phase)

`MemoryStore` / `memory.jsonl` and the `memory_store` / `memory_query` tools are untouched.
Auto-capture publishes graph records **in addition to** existing chunk indexing, so no
behavior regresses. Unifying the two layers (graph-derived embeddings replacing the
per-workflow store) is explicitly deferred — see §9.

### 4.4 Concurrency

Single-writer atomic file publishes; last-write-wins on the derived index; rebuild-on-open
reconciles external writers. This matches zerochain's existing filesystem-first concurrency
model (marker files, atomic writes) and is acceptable at contribution counts in the
hundreds–thousands.

## 5. Engine integration (auto-capture & lineage)

- `AppState` gains a workspace-level `graph: GraphStore`, opened lazily like CAS
  (`crates/zerochain-engine/src/state.rs`).
- **Workflow init publishes a `setup` node.** Body = task brief (task file contents or
  empty). Parents = declared cross-run lineage. Declaration paths, all plumbing into
  `InitWorkflowParams`:
  - CLI: `zerochain init --parent <id>` (repeatable)
  - Task frontmatter: `parents: [<id>, ...]`
  - HTTP: `parents` field on the init request body
  This gives every workflow a root and mirrors Agora's `program.md`-as-first-node.
- **Stage completion publishes a `result` node** when the stage's `CONTEXT.md` frontmatter
  has `index_output: true` (existing flag — semantics upgrade from "index chunks" to
  "index chunks + publish a typed record"). Fields:
  - `parents`: the workflow's most recent contribution at the time of publish — i.e. the
    preceding stage's `result` node, walking back past any stages with `index_output: false`;
    the workflow's `setup` node when no earlier contribution exists.
  - `body`: stage output summary (the `result.md` body).
  - `artifacts`: CAS CID of `result.md` when CAS is enabled.
  - `metric`: from a new optional stage frontmatter field
    `metric: {name: <str>, value: <f64>, direction: lower|higher}`
    (added to `ContextFrontmatter`, `crates/zerochain-core/src/frontmatter.rs`).
- **jj audit:** graph writes join the existing `jj::auto_commit` call sites (workflow init,
  stage transitions — `crates/zerochain-core/src/jj.rs`). If the workspace is not a jj
  repo the graph works unchanged; it simply has no audit layer — identical semantics to
  zerochain today.

## 6. Agent tools, CLI, HTTP

### 6.1 LLM tools (`zerochain-tools`, exposed via stage `tools:` frontmatter)

Context (`workflow`, `stage`, actor) is injected by the engine's tool driver, following the
existing `memory_store_path` injection pattern (`crates/zerochain-engine/src/tool_driver.rs`).

| Tool | Args | Behavior |
|---|---|---|
| `contribute` | `type` (insight\|hypothesis\|report), `body`, optional `parents`, `tags`, optional `metric` | Publishes record with injected provenance; returns `{id}` |
| `verify` | `target`, `verdict` (confirmed\|partial\|failed), `body` | Publishes `verification` record; supersession computed by index; returns `{id}` |
| `graph_query` | `query` text, optional `view`, filters (`type`, `tags`, `workflow`), `top_k` | Semantic search over bodies + view filters; returns id/type/parents/actor/created/metric/excerpt per hit |

Validation lives in the store: bad type/verdict, hypothesis-with-metric, verification of a
nonexistent or verification-type target → error returned to the LLM as tool output.

### 6.2 CLI (zerochain-daemon)

- `zerochain contribute --type <t> --body <md> [--parent <id>]... [--tag <t>]... [--metric name=bpb,value=1.9,direction=lower]` — actor `human:<ZEROCHAIN_OKF_ACTOR or user>`
- `zerochain verify <target> --verdict <confirmed|partial|failed> --body <md>`
- `zerochain graph [--view recent|leaves|open-hypotheses|unverified|negative|leaders] [--json]` — read-only; default view `recent`; plain text table or `--json`

### 6.3 HTTP (zerochain-server)

- `GET /v1/graph?view=...&type=...&workflow=...` — read views
- `POST /v1/graph/contributions` — body mirrors the record frontmatter
- `POST /v1/graph/verifications` — `{target, verdict, body}`
- Same bearer auth as existing routes.

## 7. Error handling

- Atomic writes everywhere; a crash mid-publish leaves at most a `.tmp` file, never a torn record.
- Corrupt record file (bad frontmatter, unknown type): skipped with a warning during index rebuild; never fatal.
- `index_output: true` graph-publish failure must not fail the stage — log the error and keep the stage's existing behavior (chunk indexing + result.md) intact.

## 8. Testing

- **Unit (zerochain-memory):** publish + idempotent re-publish; canonical-hash stability
  (key order, unknown-field rejection); index-rebuild equivalence from files only;
  supersession (newer verdict wins per `(target, actor)`); view filters; corrupt-file
  recovery; verification validation rules.
- **Engine (zerochain-engine):** setup node on init (no parents / with `--parent`);
  result chaining across a multi-stage run (parent = previous stage's contribution);
  cross-run parents land on the setup node; metric captured from frontmatter;
  graph-publish failure does not fail the stage. Use existing `FakeLlm`/`FakeEmbed` +
  `AppState` harness patterns (`crates/zerochain-engine/src/llm_driver.rs` tests).
- **Tools:** contribute → graph_query round-trip and verify → supersession through the
  existing tool-loop integration harness (`crates/zerochain-engine/tests/`).
- **CLI:** new cases in `crates/zerochain-daemon/tests/` following `integration.rs`
  conventions (tempdir workspace, run command, assert on output/exit).

## 9. Deferred (recorded so later phases don't redesign)

1. **Evidence scoring** — weighted cross-actor downstream counts with self-citation
   exclusion (Agora Eq. 2). Needs an account model richer than today's actor string.
2. **Diversity engine** — semantic clustering, monoculture detection, UCB-ranked
   exploit / explore-known / explore-novel slots (Agora §3.3). Schema already carries
   everything it needs (type, parents, actor, metric, tags).
3. **Memory unification** — replace per-workflow `memory.jsonl` with graph-derived
   embeddings; re-target `memory_query`.
4. **Matched evaluation** (Agora Appendix C) — out of scope entirely.

## 10. Non-goals for this phase

- No databases (no SQLite; the derived index stays files + in-memory).
- No changes to workflow execution semantics, stage markers, locks, or snapshots.
- No changes to OKF export format.
- No multi-workflow write coordination beyond atomic file semantics.
