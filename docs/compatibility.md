# Client and host validation

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

## Orchestrators

No native Herdr, Orca or Stackbite integration has been exercised for this public
release. Their names indicate possible host environments, not a bundled adapter
or endorsement. The coordinator can use the MCP decision surface; the host must
provide its pool and own discovery, dispatch, state and receipts.
See [the integration recipe](model-pools.md).
