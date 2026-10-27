# Switchbite

**The decision layer for your agents.** By [BiteFoundry](https://github.com/BiteFoundry).

A Rust workflow engine and MCP server for coordinating work across a factual
worker/model pool. It routes ready tasks, tracks dependencies and evidence, and
can run configured local Codex and Claude workers through verification and
integration. The coordinator supplies plans and handles actions that need its
judgment or context.

**v0.8.6-review.1 preview · Windows x64 · MIT · No Python runtime.**

Run `switchbite workflow app` to enter a Git project path and goal in the local web
app. Select which detected CLI tools may participate; the saved selection limits
both the coordinator and every task's routing catalog. A selected native CLI
coordinator supplies the plan and bounded repairs;
the existing judge and workflow engine route, execute, verify and integrate the
tasks. The completed run shows its result worktree, branch and final commit.
See [the web workflow guide](docs/workflow-ui.ko.md).

The live output panel follows coordinator, worker and verification output. It
shows elapsed time, last output and owned-process status, with explicit controls
to resume a paused run or interrupt one worker for a bounded repair attempt.

Connect a Stackbite coordinator to the project-scoped workflow server:

```powershell
.\bin\switchbite.exe stackbite --judge jev --workspace C:\path\to\project --client both
```

Start a new coordinator session in the project. The installer registers
`switchbite-stackbite` with the three-tool workflow profile. The default Jev 1.14
judge reads `TYPESAFE_API_KEY` from the coordinator environment; the installer
stores a variable reference, not the key value. Local Laya is selected with
`--judge laya --endpoint http://127.0.0.1:11435 --model laya:multilingual`.
Judge selection does not silently fall back to another provider. See the
[Stackbite guide](docs/stackbite.ko.md) and [Laya setup](docs/stackbite-laya.ko.md).

## Run a workflow

Call `switchbite_start` with an absolute workspace and a goal or plan. Omit
`pool` to discover installed Claude, Codex, Cursor and OpenCode targets. Each
ready task gets a shortlist of up to 16 candidates from the larger catalog;
Jev/Laya sees acceptance, outputs, upstream results, model and effort facts.
Use `switchbite models` to inspect the catalog. An explicit `pool` selects the
existing manual mode. An optional local `host` configures the Git base and timeout.
The engine generates run and action IDs and applies scheduling defaults. A plan
contains task IDs, objectives, dependencies, acceptance checks, and owned
`write_paths`; artifact references can go in `outputs`.

Call `switchbite_next` once to start the background driver for a known plan.
Poll `switchbite_next` with `operation: status` to see progress. Independent work
runs within resource and path reservations; verification and integration release
dependent tasks. If the workflow requests `plan`, `repair`, or `refresh_pool`,
the coordinator supplies the result using `switchbite_observe` and calls next to
continue. Unknown execution is inspected before any retry. When neither local host
configuration nor complete worker metadata selects native execution, the coordinator handles the returned
host actions and records their results. See the [coordinator guide](docs/coordinator.md)
and [automatic pool guide](docs/automatic-pools.ko.md).

| Workflow tool | Purpose |
| --- | --- |
| `switchbite_start` | Start a run with a goal or plan and a factual pool. |
| `switchbite_next` | Advance or inspect, pause, resume, and cancel a run. |
| `switchbite_observe` | Record a plan, repair, pool refresh, or host result. |

The local CLI also supports `workflow run --request-file FILE`, `workflow continue RUN`,
`workflow status RUN`, and `workflow view RUN`. Run these in the project
workspace; the viewer listens on loopback and shows the task graph and evidence.
[한국어 사용 안내](https://github.com/BiteFoundry/switchbite/blob/main/docs/agent-experience.ko.md)
· [Coordinator guide](https://github.com/BiteFoundry/switchbite/blob/main/docs/coordinator.md)
· [Policy and snapshots](https://github.com/BiteFoundry/switchbite/blob/main/docs/api-v2.md)
· [Evaluation](https://github.com/BiteFoundry/switchbite/blob/main/docs/evaluation.md)

## Preview validation

On 2026-10-27, automatic catalogs were read from four installed CLIs on Windows.
A three-task project used explicit discovered Claude, OpenCode and Codex targets;
all three passed their output checks and integrated. A separate live Jev probe
compared 16 candidates shortlisted from 65 discovered Claude/Codex combinations.
Cursor reported its account's weekly usage limit; its launch was not counted as
a completed task. See [qualification](docs/model-pool-validation.json).
## Connect once

Download the ZIP and SHA256SUMS from [Releases](https://github.com/BiteFoundry/switchbite/releases), verify and extract.

    .\bin\switchbite.exe codex

Or use `switchbite claude`, `switchbite cursor`, `switchbite grok`,
`switchbite antigravity` or `switchbite opencode` when the binary is on PATH.
Each command connects that client for the current user. Open a new client session
and approve Switchbite once if the client asks. Routing instructions then load
automatically. You do not need to edit project instruction files.

The command keeps a content-addressed executable under
`%LOCALAPPDATA%\Switchbite\bin`, so moving the extracted ZIP does not break MCP.
It merges only Switchbite's server settings, preserves other servers/settings and
comments, and saves an exact adjacent `.switchbite-*.bak` before an update.
Repeat the command safely to reconnect or upgrade. Existing managed entries keep
custom fields and explicit credentials; a different server using the same name
is a conflict rather than an overwrite. No PATH or shell-profile changes occur.

`--dry-run` previews the destination without writes; `--json` returns a short
machine-readable result. `--config-file FILE` selects an effective settings file
explicitly. Client policies and project overrides still apply. OpenCode 2 inline
configuration/directory overrides need explicit source selection, so setup refuses
to claim a connection by writing a shadowed global file.

Judge selection and credentials are separate: connection does not select or
change a judge, save key values, discover accounts or expand the worker pool.
See [connection behavior and config locations](docs/connection.md).
For a generic host or manual integration, `setup --client NAME` remains print-only;
`config --client NAME [--executable ABSOLUTE_PATH]` prints only the MCP snippet.

The generic connection commands above retain the legacy agent profile. It supplies coordinator instructions automatically and
exposes only four tools:

| Tool | When to use it |
| --- | --- |
| switchbite_route | Recommend a target for one newly ready task. |
| switchbite_validate_plan | Validate a supplied DAG. |
| switchbite_feedback | Normalize an observed outcome and evidence. |
| switchbite_status | Check version and judge configuration presence. |

The default online judge remains **Jev 1.14** (jev-1.14.0). Local Laya is selected
explicitly with `--judge laya --endpoint http://127.0.0.1:11435 --model laya:multilingual`
on `serve --profile workflow`, `serve --profile agent`, `workflow`, `route`, `invoke`, `doctor`, or `probe`. No TypeSafe key
is read by that transport. `probe --judge laya` checks a synthetic local decision
without executing workers. `--offline` still means no judge inference, including
local inference; select Laya explicitly for local-only operation.

For Jev, forward TYPESAFE_API_KEY only through
host-authorized environment controls. API access/usage are separate from the MIT
software license. --offline in setup/config prevents online judging even if the
parent has a key. Status does not prove authentication or quota.

## Route one task

The normal input requires task and pool:

    {
      "task": {"id": "csv:parse", "objective": "Implement CSV validation and regression tests"},
      "pool": [{
        "id": "worker:implementation",
        "description": "A host-authorized Rust implementation worker",
        "capabilities": ["code.change"],
        "available": true
      }]
    }

Read status, next_action, summary and candidate_id first. MCP text gives a short
summary; structured output contains score breakdowns, reasons, alternatives,
exclusions and the input snapshot. No digest calculation or replay envelope is
required from the coordinator. Only recheck_and_dispatch proposes that the host
recheck current permissions/capacity, reserve and dispatch. Honor every hold.

A candidate is one harness/model/settings/worker-instance combination. Multiple
profiles of one CLI get distinct IDs. Supply actual capabilities and availability;
Switchbite does not discover accounts, models or quotas or invent those facts.
The limit is 16 candidates per decision. The ZIP includes examples/agent-minimal.json.

## Add policy only when needed

- Profiles: balanced, quick, implementation, review, mockup.
- Configurable weights and fit/context/concentration thresholds.
- Exact capability/availability gates, explicit exclusions and producer exclusion.
- Hard cost/latency limits using host estimates; unknown estimates do not satisfy a limit.
- Ties hold by default. Explicit policies use stable ID, observed cost or latency.
- Continuity uses switch penalties/margins. Explicit has_context=false disables
  affinity; running work holds and is never automatically reassigned.

Profiles tune policy, not worker capabilities. Confidence is concentration, not
calibrated task-success probability. Recommendations are not optimality proofs.

Advanced native replay requires an exact normalized-input SHA-256 binding and
unexpired validity. Changes to task, pool, policy or continuity refuse the replay.
The binding detects stale inputs, not forged provenance; the host owns trust.
API1's historical replay contract remains available for compatibility.

## Bounded operation and evaluation

Agent defaults: two concurrent decisions, sixteen queued, 5s queue wait and 30s
total deadline. Queue wait consumes the deadline. Cancellation/timeout never frees
a still-running blocking provider slot early. Judge errors carry static codes,
retry guidance and bounded cooldowns. The legacy agent profile does not execute workers. HTTP connections are reused; fixed HTTPS endpoint, TLS,
no redirects/proxies and credential guards remain in place.

    .\bin\switchbite.exe route --request-file examples/agent-minimal.json
    .\bin\switchbite.exe snapshot --request-file examples/agent-minimal.json
    .\bin\switchbite.exe evaluate --request-file examples/evaluation-synthetic.json
    .\bin\switchbite.exe guide

Evaluation imports recorded decisions and outcomes. It reports agreement, holds,
success, time, quota and rework with missing coverage and paired manual/strongest
baseline comparisons. Synthetic fixtures verify the evaluator, not real model
quality. Evaluation does not launch workers, verify receipts, auto-learn weights
or change live policy.

## Compatibility and build

No arguments still serves docking API1 (describe/invoke). serve --profile cli
retains the original switchbite_decide tools. Generic client connections use
the legacy four-tool agent profile; the Stackbite installer selects the three-tool
workflow profile. Existing installations are not rewritten until updated.

Windows x64 MSVC is the packaged target. The manifest declares Rust 1.88;
release builds use 1.99.1 and CI checks the minimum compiler separately.

    cargo test --locked
    cargo clippy --all-targets --locked -- -D warnings
    cargo build --release --locked
    cargo run --release --locked --bin package-plugin -- --binary target/release/switchbite.exe --output dist/switchbite-0.7.0-workflow.1-windows-x64.zip

The deterministic schema-2 ZIP verifies its executable and includes schemas,
guides, examples, MIT license and dependency/Rust library notices. It is MCP
software, not a Claude/Codex-specific plugin-manager bundle or an OS sandbox.
Domain JSON is bounded to 64 KiB with strict schemas and duplicate-key protection.

Local workflow execution uses configured Codex or Claude worker executables and a
project-owned Git integration workspace. The coordinator supplies factual worker
profiles; account and quota discovery are not automatic.

## License

MIT. See LICENSE, THIRD_PARTY_NOTICES.html and RUST_STANDARD_LIBRARY_NOTICES.html.
No Stackbite host code is included.
