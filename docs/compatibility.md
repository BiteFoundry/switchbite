# Client and host validation

This page preserves v0.3 acceptance evidence. v0.4 uses the agent profile by
default; its release-specific checks are recorded in
[the v0.4 report](release-validation-v0.4.0.json).

The configuration renderer supports Claude Code, Codex, Cursor, Grok,
Antigravity, OpenCode 2 and generic MCP clients. It prints snippets only.
No user-wide client configuration is changed by tests or packaging.

## Previous prototype evidence — 2026-10-23, Windows

The same Rust decision/protocol implementation was tested before the public
Switchbite rename. Those runs used the former `jev-router` server/tool names;
they are not represented as fresh tests of the renamed release.

| Client | Installed version | Observed scope |
| --- | --- | --- |
| Claude Code | 2.1.308 | All four tools invoked; live Jev 1.14 decision; ZIP executable under a path with spaces. |
| Codex | 0.163.0 | All four tools invoked; live Jev 1.14 decision; ZIP executable under a path with spaces. |
| Cursor CLI | 2026.10.19-6bbf2c4 | MCP tool discovery and input schemas after fixture-server approval. |
| Grok CLI | 1.0.60 | MCP handshake, four tools, healthy doctor result in a trusted fixture. |
| Antigravity | 1.4.5 | Actual offline status/feedback calls from a workspace-scoped print session. |
| OpenCode | 2.1.2 | MCP server connected after child-only config override isolation. |

Cursor/Grok/OpenCode agent tool invocation was not tested. Antigravity online
key forwarding and API access were not tested. Client trust and environment
inheritance are owned by the client; status only shows judge configuration.

## Public release validation

The renamed source has native CLI/MCP lifecycle tests, complete typed schemas,
configuration rendering tests and deterministic packaging checks. Model-pool
regressions exercise distinct profiles in one harness, availability/capability
exclusion, exact target identity, opaque metadata and changed-pool replay refusal.
Release-specific checks are recorded in `docs/release-validation.json`.

For the renamed Windows release candidate, Claude and Codex invoked all four
tools with structured results and the model-pool replay. Antigravity invoked
offline status/feedback; Cursor and Grok discovered all four tools; OpenCode
reported connected. A separate native live Jev call evaluated the synthetic
model pool successfully. The executable was extracted from the local ZIP under
a path with spaces. These checks do not execute proposed workers or measure
their quality. Cursor/OpenCode fixture folders needed their own Git roots for
the intended nested workspace configuration to be discovered.

## Orchestrators

No native Herdr, Orca or Stackbite integration has been exercised for this public
release. Their names indicate possible host environments, not a bundled adapter
or endorsement. The coordinator can use the MCP decision surface; the host must
provide its pool and own discovery, dispatch, state and receipts.
See [the integration recipe](model-pools.md).
