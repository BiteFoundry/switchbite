# SwitchBite StackBite execution adapter

Automatic release following builds this adapter as an independent Cargo project
under SwitchBite's runtime home. The official source commit is exported to a
separate snapshot and consumed as a path dependency, without editing the source
checkout. Build-time `SWITCHBITE_STACKBITE_VERSION` and
`SWITCHBITE_STACKBITE_COMMIT` record the exact release identity in `capabilities`.
`self-test` checks the actual child transport's stdin, both output streams, EOFs
and exit status before the new engine becomes eligible for new runs.

See [automatic updates](../../docs/stackbite-auto-update.ko.md). The remaining
description records the original isolated-clone build and legacy Seat server.

License provenance: this crate is built in the StackBite source workspace and
inherits its `LicenseRef-Proprietary` designation. SwitchBite's MIT license does
not change the license designation of the bundled StackBite component.

Private opt-in adapter built inside the independent Stackbite source clone.
Reuses `stackbite-provider-stream`'s owned `ChildTransport`, bounded
`NdjsonFramer`, and `ExecutionBinding`. It does not read installed Stackbite
settings, discover live Rooms, use native hooks, or control existing processes.

The `capabilities` and `exec --request <path>` commands provide the separate
SwitchBite process bridge. Protocol 1 advertises Codex, Claude, Cursor,
OpenCode and Antigravity, matching SwitchBite's native execution adapters;
Grok is not an execution adapter. The host supplies the exact native command
and working directory, owns cancellation and output limits, and verifies each
provider's receipt. The bridge forwards the child stdout and stderr unchanged
through `ChildTransport`. This process bridge does not use the Seat server
or the Claude-only `structured_one_shot` protocol described below.

```powershell
switchbite-stackbite-engine serve --state C:\owned\new-state --claude-exe C:\explicit\claude.exe --cwd C:\owned\project --port 0
$env:SWITCHBITE_BACKEND_STATE = 'C:\owned\new-state'
switchbite-stackbite-engine agent context --format compact
switchbite-stackbite-engine agent seats --output json
```

The state parent must exist and the state directory must be new. An existing
root is refused, including after daemon exit: previous intent is never replayed.
`endpoint.json` holds a random bearer token for a random loopback TCP port. Keep
the directory private. Readiness stdout and public receipts never include that
token. All client commands require the explicit state environment variable;
there is no installed-state fallback. Commands accept Studio's `start`,
`launch-status`, `send` and `wait` arguments and optional `agent` prefix.

`start` reserves a fresh Seat/generation and acknowledges backend readiness.
It emits `runtime_mode: structured_one_shot`, `backend_ready: true`,
`acknowledgement_kind: backend_ready`, `onboarding_acknowledged: false`.
No model is invoked until one explicit `send`. Each Seat executes one prompt.
Duplicate request IDs with the exact same reservation/prompt return the same
receipt. Changed payloads and second prompts are refused. SHA256 prompt digests
are published; raw prompts remain only in daemon memory and the owned provider's
shell-free positional argument. Prompts must be UTF-8, nonempty, at most 16 KiB,
and contain no NUL. The cap also keeps Windows argv below its process limit.

The fresh Claude invocation uses print/stream JSON, a fresh UUID, no session
persistence, safe mode, restricted mode, disabled slash commands/hooks, strict
empty MCP configuration, `acceptEdits`, and permission prompts set to `none`.
Only Read/Write/Edit are available. Claude's `--restricted` contract confines
file tools to the declared cwd; this adapter does not implement an OS sandbox.
The system context identifies the private clone Room/Seat/generation/request,
actual cwd, available file tools, and absence of an execution API. Original
Stackbite/WorkBite/WorkStack/Switchbite/Helm and Claude orchestration environment
variables are removed; PATH includes only the explicit provider directory and
the Windows system directory. Normal provider authentication may still read the
user's provider credentials. No provider configuration is written.

Completion needs the exact init session, one exact successful result with
`is_error: false` and an explicitly empty `permission_denials`, successful exit
of the owned child, and both stdout/stderr EOFs. Foreign/duplicate/missing/error
results, tool errors, framing failures, failed exits, and refusals fail closed.
Receipts contain sanitized evidence and the observed init model, never raw
provider text. Limits: 1 MiB stdout line, 16 MiB combined output, ten-minute
execution, eight active processes, 256 reservations and sixteen connections.
Requests have byte and read/write timeout limits. Cancellation acts only on the
owned child through the existing transport.

Focused validation: `cargo test -p switchbite-stackbite-engine` and
`cargo clippy -p switchbite-stackbite-engine --all-targets -- -D warnings`.
Tests cover exact completion, both EOFs/exit, foreign/duplicate/missing results,
provider/tool errors, permission denials, token comparison and prompt digest.
No test invokes a real provider or touches installed state.
