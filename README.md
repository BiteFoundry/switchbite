# Switchbite

**The decision layer for your agents.** By [BiteFoundry](https://github.com/BiteFoundry).

A Rust library and native MCP server that recommends a target from your worker/model
pool. Give it a task and factual pool; get a clear next action, recommendation or
hold. Your host owns authorization, reservations, execution and receipts.

**v0.4.0 public preview · Windows x64 · MIT · No Python runtime.**

[한국어 사용 안내](https://github.com/BiteFoundry/switchbite/blob/main/docs/agent-experience.ko.md)
· [Coordinator guide](https://github.com/BiteFoundry/switchbite/blob/main/docs/coordinator.md)
· [Policy and snapshots](https://github.com/BiteFoundry/switchbite/blob/main/docs/api-v2.md)
· [Evaluation](https://github.com/BiteFoundry/switchbite/blob/main/docs/evaluation.md)

## Connect once

Download the ZIP and SHA256SUMS from [Releases](https://github.com/BiteFoundry/switchbite/releases), verify and extract.

    .\bin\switchbite.exe setup --client claude
    .\bin\switchbite.exe setup --client codex

Merge the printed MCP configuration into your client's effective settings, then
restart and approve the server if prompted. Setup prints configuration and next
steps; it does not modify settings. The config command prints only the snippet.
Renderers support Claude Code, Codex, Cursor, Grok, Antigravity, OpenCode 2 and a
generic MCP client. Use --executable ABSOLUTE_PATH for another install location.

The recommended agent profile supplies coordinator instructions automatically and
exposes only four tools:

| Tool | When to use it |
| --- | --- |
| switchbite_route | Recommend a target for one newly ready task. |
| switchbite_validate_plan | Validate a supplied DAG. |
| switchbite_feedback | Normalize an observed outcome and evidence. |
| switchbite_status | Check version and judge configuration presence. |

The online judge is **Jev 1.14** (jev-1.14.0). Forward TYPESAFE_API_KEY only through
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
retry guidance and bounded cooldowns. No automatic provider retries or worker
execution occur. HTTP connections are reused; fixed HTTPS endpoint, TLS,
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
retains the original switchbite_decide tools. New snippets select serve --profile
agent. Update profile arguments when migrating; installations are not rewritten.

Windows x64 MSVC is the packaged target. The manifest declares Rust 1.88;
release builds use 1.99.1 and CI checks the minimum compiler separately.

    cargo test --locked
    cargo clippy --all-targets --locked -- -D warnings
    cargo build --release --locked
    cargo run --release --locked --bin package-plugin -- --binary target/release/switchbite.exe --output dist/switchbite-0.4.0-windows-x64.zip

The deterministic schema-2 ZIP verifies its executable and includes schemas,
guides, examples, MIT license and dependency/Rust library notices. It is MCP
software, not a Claude/Codex-specific plugin-manager bundle or an OS sandbox.
Domain JSON is bounded to 64 KiB with strict schemas and duplicate-key protection.

Native Herdr/Orca/Stackbite discovery/dispatch adapters, automatic decomposition,
parallel workflow execution, global assignment optimization, alternative judge
backends and Linux/macOS release binaries are not bundled. Coordinator instructions
support the host workflow; a host must still make the actual routing calls.

## License

MIT. See LICENSE, THIRD_PARTY_NOTICES.html and RUST_STANDARD_LIBRARY_NOTICES.html.
No Stackbite host code is included.
