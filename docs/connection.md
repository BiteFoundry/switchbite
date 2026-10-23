# One-command client connection

Run `switchbite codex`, `claude`, `cursor`, `grok`, `antigravity` or `opencode`
once per client, then open a new client session. The command configures the MCP
agent profile and preserves unrelated settings. The server's initialization
instructions explain the four-tool workflow automatically. Client approval and
organization policies remain effective; configuration is not proof of a live
connection, authentication, quota or automatic execution.

## Default user files

| Client | File |
| --- | --- |
| Codex | `$CODEX_HOME/config.toml`, default `~/.codex/config.toml` |
| Claude Code | `$CLAUDE_CONFIG_DIR/.claude.json`, default `~/.claude.json` |
| Cursor | `~/.cursor/mcp.json` |
| Grok | `~/.grok/config.toml` |
| Antigravity CLI | `~/.gemini/config/mcp_config.json` |
| OpenCode 2 | `$XDG_CONFIG_HOME/opencode/opencode.json(c)`, default `~/.config/opencode/opencode.json(c)` |

OpenCode updates an existing `.jsonc` ahead of `.json`, or creates `.json`.
`OPENCODE_CONFIG` selects a file explicitly. Inline `OPENCODE_CLI_CONFIG_CONTENT`
and `OPENCODE_CONFIG_CONTENT` cannot be durably edited by this process; their
presence stops automatic global setup. Legacy `OPENCODE_CONFIG_DIR` precedence
is not established by the v2 documentation, so its presence also requires an
explicit `--config-file`. This adapter targets OpenCode 2's `mcp.servers` shape.
Project overrides can shadow a user entry; select that effective file explicitly
if needed. The connector does not enumerate or rewrite other projects.

## Preservation and recovery

The executable is copied into `%LOCALAPPDATA%/Switchbite/bin/<sha256>/switchbite.exe`.
Copies are immutable and verified by content before reuse. Old copies remain
available for other clients and backups. No automatic deletion, background service,
PATH edit, shell wrapper, worker execution or provider request is involved.

Updates preserve unknown server fields and explicit environment values. Only the
managed command/profile fields and connection enable flags are refreshed. Codex
adds the `TYPESAFE_API_KEY` variable-name grant while preserving other grants.
Claude/Cursor add their environment reference only if no explicit value exists.
`--offline` changes the server arguments to prevent online judging; it does not
erase credentials already present in an existing config.

The connector rejects duplicate JSON keys, wrong container types, malformed or
oversized files, and a foreign server named `switchbite`. A same-directory stage
file is flushed before replacement. An exact original-byte backup is saved before
each changed update. Repeat setup makes no backup when nothing changed. A lock
serializes Switchbite writers and an additional read detects external edits before
replacement; unrelated editors do not participate in that lock.

To undo an update, close the client and restore the adjacent backup. To undo a new
registration, remove only the `switchbite` server entry. If a process was interrupted,
check the PID in `.switchbite-lock` before removing a stale lock. Backups can contain
the pre-existing secrets from the original settings; keep them with the settings.

`--dry-run` writes neither configuration nor executable copies. `--json` reports
`preview`, `configured` or `already_configured`, paths, optional backup and next
action. Judge configuration is always reported as `unchanged`. Existing print-only
`setup` and `config` commands keep their previous behavior.

## Search-first decision, 2026-10-23

Existing `connection::render` already owns all six dialects; the connector reuses
it. Native registration commands were checked in Codex 0.163.0, Claude Code
2.1.308, Grok 1.0.60, Antigravity 1.4.5 and OpenCode 2.1.2. Cursor CLI
2026.10.19-6bbf2c4 exposes enable/list/login but no add command.

Native commands differ in scope, replacement and environment support. Codex add
does not expose `env_vars` forwarding. Composing them would still require config
editing, backup and conflict handling. Printing snippets keeps manual user work.
The chosen thin merger reuses the existing renderer, MIT `jsonc-parser` 0.34.0
CST and MIT/Apache-2.0 `toml_edit` 0.25.17. It avoids a new configuration grammar,
preserves comments, and makes preview/idempotency/backups consistent. JSONC has
no declared MSRV; the locked dependency graph must pass the Rust 1.88 CI check.

Primary references:

- [Codex MCP: config and initialization instructions](https://learn.chatgpt.com/docs/extend/mcp)
- [Claude MCP scopes](https://code.claude.com/docs/en/mcp)
- [Claude config directory](https://code.claude.com/docs/en/mcp-quickstart)
- [Cursor MCP](https://prod.cursor.com/docs/mcp)
- [Antigravity MCP, CLI tab](https://antigravity.google/docs/mcp)
- [OpenCode 2 configuration](https://opencode.ai/v2/docs/config/)
- [jsonc-parser CST](https://docs.rs/jsonc-parser/0.34.0/jsonc_parser/cst/)
- [toml_edit](https://docs.rs/toml_edit/0.25.17/toml_edit/)
