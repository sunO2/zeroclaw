# Multi-agent runtime (v0.8.0)

This page documents the architecture and operator-facing surface of the multi-agent runtime that landed in v0.8.0 (#6272). The doc is intentionally short — for the schema-level field reference, see [Config](../reference/config.md); for live setup steps, see [Multi-agent setup](../contributing/multi-agent-setup.md).

## Vocabulary

- **Install dir** — the directory holding everything ZeroClaw owns on a host. Typically `~/.zeroclaw/`. Equivalent to the dir containing `config.toml`.
- **Agent** — a configured `[agents.<alias>]` block: a join table of references (risk_profile, model_provider, channels), a per-agent workspace dir, and a per-agent memory backend selection. Each agent picks one memory backend at creation; the choice is immutable in v0.8.0.
- **Aliased workspace** — `<install>/agents/<alias>/workspace/`. One per agent. Holds the agent's identity files (`AGENTS.md`, `SOUL.md`, `IDENTITY.md`, `USER.md`, `BOOTSTRAP.md`, `MEMORY.md`) and any operator data the agent owns. Replaces the v0.7.x single `<install>/workspace/`.
- **SubAgent** — a runtime-spawned ephemeral sub-agent that inherits its parent's identity, security policy, and memory allowlist. Two spawn sites: the cron `JobType::Agent` dispatch and the agent-loop `spawn_subagent` tool. SubAgents cannot escalate beyond the parent's permissions.
- **Peer group** — a `[peer_groups.<name>]` block declaring an opt-in cross-agent communication set on a single channel. Mutual membership: agents A and B are peers only when both appear in the same group's `agents` list.

## Permissions model

Each agent's effective `SecurityPolicy` is built by `SecurityPolicy::for_agent(config, alias)`:

1. Start from the agent's risk profile (`[risk_profiles.<profile>]`).
2. Set the boundary to the per-agent workspace dir (`<install>/agents/<alias>/workspace/`).
3. Walk `[agents.<alias>.workspace.access]`:
   - `Read` → sibling's workspace lands in the read-only allowlist.
   - `Write` / `ReadWrite` → sibling's workspace lands in the read-write allowlist.
4. If `[agents.<alias>.workspace.unrestricted_filesystem]` is `true`, flip `workspace_only` off.

The read-only allowlist is honored by `file_read` (and other read-side tools); the read-write allowlist gates `file_write`, `file_edit`, `git_operations`, and the shell tool's path-touching invocations. POSIX device files (`/dev/null`, `/dev/zero`, `/dev/random`, `/dev/urandom`) are always readable so shell idioms keep working without per-agent config.

SubAgent spawns enforce the rule that a child cannot escalate beyond its parent: `SecurityPolicy::ensure_no_escalation_beyond` runs at spawn time and rejects any policy override that adds paths, commands, or budgets the parent doesn't have. The rejection is wrapped with the precise `EscalationViolation` so diagnostics name the offending field.

## Memory model

Each agent has its own `Arc<dyn Memory>` instance. The factory (`zeroclaw_memory::create_memory_for_agent`) dispatches by backend kind:

- **SQLite / Postgres / Lucid**: shared install-wide store. The `agents` table maps alias → UUID, and the `memories` table carries `agent_id` referencing that UUID. The factory wraps the inner backend in `AgentScopedMemory`, which stamps the bound agent's UUID on every store via `store_with_agent` and filters every recall via `recall_for_agents` with the resolved allowlist.
- **Markdown**: per-agent dir. Each agent's `MarkdownMemory` writes to `<install>/agents/<alias>/workspace/MEMORY.md` and `memory/YYYY-MM-DD.md`. Cross-agent recall is composed by `AgentScopedMarkdownMemory`, which holds the bound agent's `MarkdownMemory` plus a peer set of `(alias, MarkdownMemory)` pairs and unions their results with `[<alias>] ` attribution prefixes on each row.
- **Qdrant**: shared collection, payload-keyed. The `agent_id` payload field is the per-agent attribution; `recall_for_agents` over-fetches and post-filters by payload.
- **None**: no-op stub. The wrapper still exists so the runtime path is uniform.

Cross-backend cross-agent memory is **out of scope for v0.8.0**. The schema validator at config load rejects `read_memory_from` entries that point at a sibling on a different backend; deferred to v0.8.1 alongside agent-rename and backend-switching.

## v0.7.x → v0.8.0 upgrade

On first boot of v0.8.0 against a v0.7.x install:

1. **Filesystem migration** (`migrate_legacy_workspace_to_default_agent`): `<install>/workspace/` is moved into `<install>/agents/default/workspace/`. A timestamped backup at `<install>/backup-<ts>/legacy-workspace/` is written first (copy-not-rename, so a partial failure doesn't orphan the legacy data).
2. **V3 schema migration** (`schema/v2.rs`): synthesizes an `agents.default` config block when one doesn't exist; `default_temperature`, `default_model`, etc. fold into the new shape.
3. **DB migration** (`migrate_v0_8_0_multi_agent` on each backend): adds the `agents` table, inserts the `default` row with a fresh UUID, ALTERs `memories` to add `agent_id` (nullable, indexed), and backfills existing rows to the default agent's UUID.

Every step is idempotent: a re-run on an already-migrated install is a no-op. Roll-back from a partially-applied upgrade: stop the daemon, restore from the backup dir, downgrade the binary.

## Logging

Tracing-subscriber uses a custom event formatter that prefixes every log line with the active agent's alias (e.g. `[default] starting agent loop`). Lines emitted outside any agent-loop scope (boot, V3 migration, filesystem migration, scheduler poll) get a `[system]` prefix. `grep '\[<alias>\]' zeroclaw.log` isolates one agent's activity in a multi-agent install.

The agent-loop entry binds `agent_alias` as a tracing-span field; SubAgent spawn sites bind `parent_alias` so their nested spans carry attribution to the merged log stream. The structured sinks (otel, dora, prometheus) emit `agent_alias` as a label without further per-agent code paths.

## CLI

- `zeroclaw agent ...` (singular) — runs an agent.
- `zeroclaw agents create <alias> --risk-profile <name> [--memory-backend <kind>]` — writes a new `[agents.<alias>]` block, creates the workspace dir, seeds bootstrap identity files.
- `zeroclaw agents delete <alias> [--yes] [--dry-run]` — drops the config block, strips the alias from peer-group memberships, removes the workspace dir. `--dry-run` prints the impact set without touching anything.
- `zeroclaw agents list` — prints configured aliases with risk-profile, model-provider, memory-backend, and channel summary.

The `agents delete` active-session refusal (the issue's "refuse on in-flight sessions" guard) lands in v0.8.1 alongside the runtime session registry. v0.8.0 protection is the `--yes`/`--dry-run` flags.

## Out of scope for v0.8.0

Documented in the issue's [non-goals list](https://github.com/zeroclaw-labs/zeroclaw/issues/6272) and tracked separately for v0.8.1:

- Cross-backend cross-agent memory access (e.g. SQLite agent reading a Postgres agent's rows).
- Agent rename. The `agents.id` UUID indirection is the rename-ready foundation.
- Pre-delete archive and restore.
- Per-agent secret namespacing. Single workspace-wide `SecretStore` stays unchanged in v0.8.0.
- Lucid wire-format extensions for cross-agent scoping.
- Active-session refusal on `agents delete`.
