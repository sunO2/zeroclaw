# Multi-agent setup walkthrough (v0.8.0+)

This is the operator-side companion to the [multi-agent architecture page](../architecture/multi-agent.md). Follow it to add a second agent to an install, configure cross-agent memory access, and put both agents in a peer group on the same channel.

Background: each agent has its own workspace dir at `<install>/agents/<alias>/workspace/`, picks one memory backend at creation (immutable), and is gated by a `[risk_profiles.<profile>]` entry. The default agent (created by the v0.7.x→v0.8.0 upgrade) is just one entry on this list — there is no special "default" code path at runtime.

## Prerequisites

- v0.8.0 or later running against the install. If upgrading from v0.7.x, run `zeroclaw config migrate` once to lock the V3 schema migration to disk; the filesystem migration runs automatically on first boot.
- A `[risk_profiles.<name>]` entry the new agent will inherit. The default agent's profile (`risk_profiles.default`) is fine for most uses.

## Add a second agent

```bash
zeroclaw agents create researcher \
    --risk-profile default \
    --memory-backend sqlite
```

This:

1. Writes a new `[agents.researcher]` block to `config.toml` with `enabled = true`, the supplied risk profile, and `memory.backend = "sqlite"`.
2. Creates `<install>/agents/researcher/workspace/`.
3. Seeds default identity files (`AGENTS.md`, `SOUL.md`, `IDENTITY.md`, `USER.md`, `TOOLS.md`, `BOOTSTRAP.md`) so the agent has a basic identity to load on its first run.

Edit those identity files to give the agent its persona; the agent loop reads them on every start.

## Bind a channel

The created agent has no channels by default — without one, it has nowhere to listen. Edit `config.toml`:

```toml
[agents.researcher]
enabled = true
risk_profile = "default"
channels = ["telegram.prod"]   # must reference a configured [channels.telegram.prod]

[agents.researcher.memory]
backend = "sqlite"

[agents.researcher.workspace]
# `path` defaults to <install>/agents/researcher/workspace/
```

Save and restart the daemon. The agent picks up its channel on next start.

## Cross-agent file access

By default, an agent can only read and write within its own workspace dir. To grant `researcher` write access to the `default` agent's workspace and read access to a third `archivist` agent's:

```toml
[agents.researcher.workspace.access]
default = "write"
archivist = "read"
```

Effective behavior:

- `file_read` from `researcher` can read both `<install>/agents/default/workspace/` and `<install>/agents/archivist/workspace/`.
- `file_write` and `file_edit` from `researcher` can write into `<install>/agents/default/workspace/` but **not** `<install>/agents/archivist/workspace/`.

POSIX device files (`/dev/null`, `/dev/zero`, `/dev/random`, `/dev/urandom`) are always readable, no per-agent config needed.

## Cross-agent memory access

Same-backend only in v0.8.0. To let `researcher` recall memories that `default` wrote, both agents must use the same memory backend (e.g. both `sqlite`):

```toml
[agents.researcher.workspace]
read_memory_from = ["default"]
```

The schema validator rejects entries that point at a sibling on a different backend — the runtime never sees a cross-backend allowlist by the time it builds the per-agent memory wrapper.

The bound agent always sees its own rows; the allowlist is purely additive. There is no way to *hide* an agent's own rows from itself in v0.8.0.

## Peer group on a shared channel

Two agents become "peers" (each can address the other on a channel) only when **both** appear in the same `[peer_groups.<name>]` block:

```toml
[peer_groups.research]
channel = "telegram.prod"
agents = ["default", "researcher"]
external_peers = [
    { username = "operator" },
]
ignore = []
```

`external_peers` lists humans or external bots the group expects on the same channel; the runtime accepts inbound from those usernames as cross-agent traffic. `ignore` is a per-group blocklist that subtracts from the resolved peer set every member sees — useful for excluding a specific bot account that's noisy.

The schema validator at config load enforces:

- Every member's `channels` list includes the group's `channel` (an agent that doesn't listen there can't peer there).
- Every member is a configured agent (no dangling references).
- `read_memory_from` does not point at the agent itself.

## Inspect the install

```bash
zeroclaw agents list
```

Prints every configured agent with its risk profile, model provider, memory backend, and channel set. Useful before `agents delete <alias>` to see what's wired up.

## Delete an agent

```bash
zeroclaw agents delete researcher --dry-run
# review the impact set, then:
zeroclaw agents delete researcher --yes
```

`--dry-run` prints the impact set (config block, workspace dir, peer-group memberships) without touching anything. Without `--yes` the command requires an interactive `[y/N]` confirm. On confirm:

- The `[agents.researcher]` block is removed from `config.toml`.
- Every `[peer_groups.<name>]` that listed `researcher` has the alias stripped (memberships rewrite, the group itself stays).
- `<install>/agents/researcher/workspace/` is removed.

The agent's memory rows in the shared SQLite/Postgres store are **not** automatically purged — they keep their `agent_id = <researcher-uuid>` attribution, but no live agent maps to that UUID anymore. Manual cleanup if desired:

```sql
DELETE FROM memories WHERE agent_id = (SELECT id FROM agents WHERE alias = 'researcher');
DELETE FROM agents WHERE alias = 'researcher';
```

(Per-agent memory data deletion lands as a v0.8.1 follow-up; v0.8.0 leaves the rows in place so an operator can recover them if a delete was a mistake.)

## Verify

Look at the merged log stream — every line should now carry `[<alias>]` or `[system]` prefixes:

```bash
zeroclaw daemon 2>&1 | grep '\[researcher\]'   # researcher's lines only
zeroclaw daemon 2>&1 | grep '\[system\]'       # boot/migration/scheduler lines only
```

If the boundary checks are working, `file_read /dev/null` from any agent succeeds (POSIX device-file allowlist), `file_read` outside the workspace + access list fails with `Path escapes workspace directory`, and `file_write` to a read-only allowlisted sibling fails with the same message.
