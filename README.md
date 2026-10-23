# Switchbite

**The decision layer for your agents.** By [BiteFoundry](https://github.com/BiteFoundry).

Switchbite is a Rust library and native MCP stdio server that recommends a worker
from a caller-supplied pool. A candidate can be an agent, a model, or a specific
harness/model/settings combination. Supply a task, factual capabilities,
availability and preferences; receive an assignment proposal or an explicit hold.

It also validates supplied task DAGs and normalizes outcome feedback. The host
owns authorization, process execution, state, receipts and orchestration.

**v0.3.0 is the first public preview.** The release binary targets Windows x64.
No Python runtime is needed. Linux and macOS builds are not release-validated.

[한국어 안내](https://github.com/BiteFoundry/switchbite/blob/main/docs/quickstart.ko.md)
· [Model pools and orchestrators](https://github.com/BiteFoundry/switchbite/blob/main/docs/model-pools.md)
· [API details](https://github.com/BiteFoundry/switchbite/blob/main/docs/api.md)
· [Client validation](https://github.com/BiteFoundry/switchbite/blob/main/docs/compatibility.md)

## Try it

Download the Windows ZIP and SHA-256 checksums from
[Releases](https://github.com/BiteFoundry/switchbite/releases), then extract it.

```powershell
.\bin\switchbite.exe --version
.\bin\switchbite.exe config --client claude
.\bin\switchbite.exe config --client codex
```

The config command prints a snippet; merge it into your client's MCP settings.
It does not edit settings. Supported renderers: `claude`, `codex`, `cursor`,
`grok`, `antigravity`, `opencode` (version 2), and `generic`.
Use `--executable ABSOLUTE_PATH` for another location or `--offline` to disable
online judging even when the parent process has a key.

For online decisions, make `TYPESAFE_API_KEY` available to the MCP child process
through your host's environment controls. The included decision backend is
**Jev 1.14** (`jev-1.14.0`) from TypeSafe. The plugin is MIT-licensed; API access
and usage are separate from the software license. Offline replay, DAG validation
and feedback do not need a key.

## MCP interface

Client snippets launch `switchbite.exe serve --profile cli`:

| Tool | Result |
| --- | --- |
| `switchbite_decide` | Rank supplied candidates and propose an assignment or hold. |
| `switchbite_validate_plan` | Validate a supplied DAG and return a topological proposal. |
| `switchbite_feedback` | Normalize an observed outcome and evidence into an event proposal. |
| `switchbite_status` | Report version and judge configuration presence, without secrets or network calls. |

All four tools provide input/output JSON schemas. API version defaults to 1;
omitted task/candidate extensions default to `{}`. Capability and availability
facts must come from the host. Status does not prove authentication or quota.

For a small generic docking surface, launch with no arguments: `describe()`
returns the operation schemas; `invoke(operation, request)` accepts `decide`,
`validate_plan`, or `feedback`. Successful `structuredContent` is the domain result
itself. The same implementation can be embedded through `switchbite::Plugin`.

## Use a model pool

The same decision operation works inside an orchestrator. Represent each
executable target as a distinct candidate, for example `pool:codex:review` and
`pool:claude:implementation`. Map the returned exact candidate ID to your host's
launch target. See the complete
[model-pool example](https://github.com/BiteFoundry/switchbite/blob/main/examples/model-pool-replay.json).

Switchbite first excludes unavailable candidates and those missing any required
capability. It then ranks eligible candidates with `0.7 * fit + 0.3 * preference`
using typed Jev judgments and fixed hold thresholds. This is a recommendation
under the supplied information, not a guarantee of globally optimal outcomes.

Herdr, Orca and Stackbite can be integration hosts. This release does **not** ship
their native model discovery, quota discovery, dispatch adapters, automatic task
decomposition, parallel DAG execution, cost/latency optimization or feedback
learning. Opaque `extensions` are for host mapping and are not scored or sent to
Jev. A Rust `Judge` can be injected, but the public response contract currently
accepts only `jev-1.14.0`; other decision backends are not bundled.

## Build and test

Windows x64 MSVC tools and Rust/Cargo are required for the packaged release.
The manifest's minimum Rust version is 1.88; release validation uses 1.99.1.
`Cargo.lock` pins dependencies, including the official `rmcp` MCP SDK.

```powershell
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
cargo build --release --locked
cargo run --release --locked --bin switchbite -- invoke decide --request-file examples/model-pool-replay.json
cargo run --release --locked --bin package-plugin -- --binary target/release/switchbite.exe --output dist/switchbite-0.3.0-windows-x64.zip
```

The packager verifies an x64 PE binary, runs its offline `describe`, checks the
embedded contract, and produces a deterministic ZIP without replacing an existing
artifact. The archive contains `plugin.json`, `bin/switchbite.exe`, the schema,
README, MIT license and dependency notices. The schema-2 manifest is optional host
packaging metadata; connecting a CLI over MCP does not require a manifest loader.
It is not a Claude/Codex-specific plugin-manager bundle.

## Data boundary

The executable does not launch workers, edit host configuration or persist
feedback. Jev receives the task, eligible candidate descriptions/capabilities and
preferences. `context_refs` are strings, not files fetched by Switchbite; include
the necessary non-secret context in the objective. The host decides what to send.

HTTP uses the fixed HTTPS TypeSafe endpoint with certificate verification, no
redirects, automatic retries or ambient proxies, a 10-second connect timeout and
30-second total timeout. Domain JSON is limited to 64 KiB with strict schemas,
duplicate-key protection and finite numbers. API failures return static errors.

Replay judgments take precedence over a configured online judge. The host must
bind replays to the original task and candidate facts; matching IDs alone do not
prove freshness. The request digest is an audit aid. Fit confidence measures score
distribution concentration, not calibrated task-success probability. Read-only
MCP hints do not grant host permissions or provide an OS sandbox.

## License

[MIT](https://github.com/BiteFoundry/switchbite/blob/main/LICENSE).
Dependency terms and attributions are retained in `THIRD_PARTY_NOTICES.html`
and `RUST_STANDARD_LIBRARY_NOTICES.html`.
This repository begins with a curated source snapshot of the earlier private
Rust plugin. Private development history and obsolete Python/deployment reports
are not part of this public distribution. No Stackbite host code is included.
