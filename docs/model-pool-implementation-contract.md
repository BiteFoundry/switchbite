# Automatic model pool implementation contract

Scope for the first package: W0–W6 of the Korean implementation plan. Rust plugin only. Root owns workflow integration, routing context, catalog snapshots on route actions, version/release and final acceptance. Three Codex GPT-6.1-Terra high workers own the independent boundaries below. Stackbite originals, private clone and existing seats are preserved.

## Catalog worker

Own `src/catalog/**`, `tests/catalog.rs`. Add no lib/main/workflow edits; root exports `catalog` at integration. Reuse existing dependencies. Public JSON entry points:

```rust
pub fn snapshot(workspace: &Path, selection: &Value) -> Result<Value, CatalogError>;
pub fn shortlist(snapshot: &Value, task: &Value, preferences: &str, limit: usize) -> Result<Vec<Value>, CatalogError>;
```

CatalogError implements Error/Display and `to_value() -> Value`. Snapshot shape: `{schema_version:1, revision:string, observed_at_ms:u64, candidates:[...], diagnostics:[{provider,code,...}]}`. Candidate facts use current workflow fields: id, description, capabilities, available, provider, executable, model, effort, resource_key, plus optional estimates/has_context. Discovery-only metadata is under `catalog_info:{source,readiness,observed_at_ms,...}` and is stripped by root before legacy validation/execution. IDs must be <=128 ASCII letters/digits/`-_.:`. Stable per harness/model/effort/account scope/host, use hashes for long IDs. Explicit CLI user models and official discovered/default aliases are eligible; invented concrete model names are not.

Selection shape `{providers?:[claude|codex|cursor|opencode], refresh?:bool}`; `{}` means supported installed providers. Persist cache under workspace `.switchbite/catalog/`, default model facts TTL24h, readinessTTL60s. Respect ARCHIVED.json; read-only CLI/model probes only, no login/config modifications. No cloud model calls, secrets or ambient Stackbite access. Bound process reads and timeout. Cache cannot become an executable-launch injection path.

Shortlist returns up to min(limit,16) candidates. Filter known unavailable/unsupported requirements; prefer user roles with balanced diversity across harness/model/effort and deterministic order. Preserve configured exact target when task declares a routing pin (root will supply task.routing object); if requested pin cannot be found return empty, never substitute. Readiness unverified is explicitly reported as such, not falsely authenticated. No candidates is a normal empty snapshot with diagnostics.

## Worker adapters worker

Own `src/adapters/mod.rs`, `src/adapters/workers/**`, adapter tests only. Preserve process.rs Job Objects, Git ownership/verification/integration and receipt recovery. Extract current Claude/Codex command/receipt code into a small common worker boundary, then implement Cursor and OpenCode from actual installed CLI help plus source references. No new dependency without coordinating.

Root needs `pub fn supports_provider(provider:&str)->bool` and `LocalHost::describe` updated to list implemented native providers. Existing LocalHost API and exact result shape remain. `effort:"default"` means use a documented CLI default/omit unsupported effort flag; explicit nondefault effort must map to a supported option or return an actionable error, never silently downgrade. Native program paths must execute without a command-shell wrapper where possible. Reuse current owned process and worktree lifecycle, redact environment keys exactly as before. Distinguish terminal completion from recoverable tool errors and ensure follow-up work sees actual outputs.

Sources: Vibe Kanban Apache2 pinned `a019cebf0693f787ef0f8dc2bb8d37edd35527ac`, executors/{mod,cursor,opencode}.rs. Prefer independently writing a thin adapter from the documented protocol to copying whole dependency graphs; copied substantive source requires provenance/NOTICE. Do not copy SUL OmO or ELv2 Paradev code.

## Connection worker

Own `src/connect.rs`, `src/connection.rs`, `src/main.rs`, `src/mcp_server.rs`, `src/workflow_interface.rs`, connection/interface-specific tests and a new `src/workspace_index.rs` if useful. Root owns lib exports; no worker changes to lib.rs. Workflow core start will accept `{workspace,goal|plan,pool? ,catalog?,...}`; explicit pool wins, omission selects catalog automatically. Existing manual pool unchanged. Core performs discovery; interface must not duplicate it.

Keep three MCP tools, make pool optional/explain auto discovery. New user-level connects launch `serve --profile workflow` and preserve the configured judge choice. Project Stackbite installer remains bound to explicit workspace. For generic workflow serve without --workspace add dynamic Interface factory (e.g. `Interface::unbound()`), choose per-run root from start request.workspace and persist a small run->workspace index under user app data. Bound interface still rejects request.workspace mismatch/canonicalizes appropriately; if compatibility requires old in-memory tests, coordinate exact boundary. Root core itself accepts whatever explicit workspace caller supplies; interface is binding owner.

Persistent run index uses read/write synchronization and atomic saves; malformed/missing run returns actionable not_found. Do not scan arbitrary workspaces or rely on MCP process cwd for generic connection. Root's Workflow root always remains workspace/.switchbite/workflow.

CLI `switchbite models [--refresh]` renders `catalog::snapshot(cwd, selection)` without keys/launching workers. Catalog module may not be present until integration; coordinate or compile with root export locally as uncommitted change only.

## Shared delivery

Implementation role only. Start a bounded patch promptly after reading these entry points; no unrelated redesign or approval/reporting framework. Apply anti-defensive-writing and avoid-ai-writing to prompts/status/docs: every change should improve connection, routing or completed output. Retain actual task ownership, cancellation and correctness checks. Work in the assigned worktree/base; don't push branches. Commit only owned files, send exact hash, checks, failures and concrete remaining work. Root integrates and independently accepts.

Search-first has already located the relevant upstream modules. Inspect existing project code before focused source lookups. No .codegraph index exists; do not create one. No verified WorkBite Task ID is available; reuse existing build outputs without storage enrollment. Use login:false. All Cargo builds share existing CARGO_TARGET_DIR `C:/Users/user/source/Switchbite/.switchbite/laya-build`; request the build slot before running Cargo. Coding and source review can proceed while another build runs.
