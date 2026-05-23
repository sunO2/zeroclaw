# Plan: ACP sessions must not interact with persistent memory

## Problem

ACP and Chat are different session types. Today they share the same agent
construction path and both have full memory access — recall, auto-save, and
all five memory tools. An ACP session (IDE-driven coding assist) should not
save "summarise the last commit" into long-term memory, and should not recall
"user likes Thai food" from a Telegram chat.

## Design contract

- **ACP sessions** inherit personality, skills, risk profile, runtime profile,
  model provider, and all non-memory tools. They do NOT interact with the
  agent's persistent memory system.
- **Chat sessions** retain full memory integration (no change).
- ACP session history lives in `acp-sessions.db` — persistent, resumable,
  deleteable, selectable. This is the session's context, not the agent's
  long-term memory.

## Changes

### P1 — Agent builder: add `exclude_memory` flag

**File:** `crates/zeroclaw-runtime/src/agent/agent.rs`

Add an `exclude_memory: bool` field to `Agent` and its builder (same pattern
as `auto_save`). Default `false`.

When `exclude_memory` is `true`:
- `memory` is set to `Arc::new(NoneMemory::new("none"))` regardless of config
- `auto_save` is forced to `false`
- `include_memory` on the `TemplateContext` is set to `false` (renders
  `AGENTS.no-memory.md` and omits `MEMORY.md` from the system prompt)

This keeps the flag close to where it takes effect and avoids scattering
ACP-awareness across the codebase.

**In `from_config_with_session_cwd_and_mcp_approval_mode` (~line 636):**

1. Accept a new `exclude_memory: bool` parameter (or add it to the builder
   chain — see below for call-site wiring).
2. When `true`:
   - Replace `let memory: Arc<dyn Memory> = zeroclaw_memory::create_memory_for_agent(...)` with `Arc::new(NoneMemory::new("none"))`.
   - Override `.auto_save(false)` on the builder.
3. The `NoneMemory` is already a no-op backend — `store` and `recall` succeed
   silently with empty results. This means `memory_loader.load_context()` in
   `turn_streamed` returns empty, and the `auto_save` guard short-circuits.
   No changes needed in `loop_.rs` or `turn_streamed`.

### P2 — Tool filtering: strip memory tools for ACP agents

**File:** `crates/zeroclaw-runtime/src/agent/agent.rs` (same builder path)

When `exclude_memory` is `true`, filter the tools list after construction
(same site as the existing `excluded_tools` filter at ~line 917):

```rust
if exclude_memory {
    const MEMORY_TOOLS: &[&str] = &[
        "memory_recall",
        "memory_store",
        "memory_forget",
        "memory_export",
        "memory_purge",
    ];
    tools.retain(|t| !MEMORY_TOOLS.contains(&t.name()));
}
```

This runs before skill registration and MCP wiring, so memory tools are
gone from the tool set before the system prompt is built.

### P3 — Wire `exclude_memory` through the ACP call chain

**Files:**
- `crates/zeroclaw-runtime/src/agent/agent.rs` — add `exclude_memory` param
  to `from_config_with_session_cwd_and_mcp_backchannel` (the ACP entry point)
  and thread it through to `from_config_with_session_cwd_and_mcp_approval_mode`
- `crates/zeroclaw-channels/src/orchestrator/acp_server.rs` — pass `true` at
  the three call sites: `handle_session_new` (~line 477),
  `handle_session_load` (~line 626), `handle_session_resume` (~line 774)

All other callers (channel orchestrator, CLI interactive, cron) continue to
pass `false` (or use the existing convenience wrappers that default to
`false`).

**Approach:** Add `exclude_memory` as a parameter to
`from_config_with_session_cwd_and_mcp_backchannel` rather than creating yet
another wrapper function. The existing non-ACP callers of the non-backchannel
path (`from_config_with_session_cwd_and_mcp`, `from_config_with_session_cwd`,
`from_config`) are unaffected — they don't go through the backchannel variant.

### P4 — Update `AGENTS.no-memory.md` template

**File:** `crates/zeroclaw-runtime/src/agent/personality_templates/AGENTS.no-memory.md`

The current template says "memory.backend = 'none' — persistent memory is
disabled" and "All context exists only within the current session." This is
accurate for ACP. However, the template also says:

> When someone says "remember this" -> update daily file or MEMORY.md

This is wrong for ACP — the agent shouldn't be writing to memory files
either. Review and adjust the template so it doesn't encourage file-based
memory workarounds. The template should communicate:

- No persistent memory in this session type
- Session history is the context — it persists across resumes
- Don't write memory files

### P5 — Update ACP docs

**File:** `docs/book/src/channels/acp.md`

Add a "Memory" section (after "Security") documenting:

- ACP sessions do not interact with the agent's persistent memory system
- Memory tools (`memory_recall`, `memory_store`, `memory_forget`,
  `memory_export`, `memory_purge`) are not available in ACP sessions
- Session context comes from the persisted conversation history
  (`acp-sessions.db`), which is resumable and deleteable
- This is a deliberate design choice: ACP is for IDE-driven coding tasks,
  not long-term relationship building

### P6 — Tests

**File:** `crates/zeroclaw-channels/src/orchestrator/acp_server.rs` (test module)

1. **`acp_agent_has_no_memory_tools`** — Create an ACP session, inspect the
   agent's tool list, assert none of the five memory tool names are present.

2. **`acp_agent_uses_none_memory`** — Create an ACP session, verify the
   agent's memory backend is `NoneMemory` (via `memory.name() == "none"`).

**File:** `crates/zeroclaw-runtime/src/agent/agent.rs` (test module)

3. **`exclude_memory_forces_none_backend_and_no_autosave`** — Build an agent
   with `exclude_memory = true` via the builder, verify `auto_save == false`
   and `memory.name() == "none"`.

4. **`exclude_memory_strips_memory_tools`** — Build an agent with
   `exclude_memory = true`, verify no `memory_*` tools in the tool set.

## Files touched

| File | Change |
|------|--------|
| `crates/zeroclaw-runtime/src/agent/agent.rs` | `exclude_memory` field, builder method, tool filtering, NoneMemory override |
| `crates/zeroclaw-channels/src/orchestrator/acp_server.rs` | Pass `exclude_memory: true` at 3 call sites |
| `crates/zeroclaw-runtime/src/agent/personality_templates/AGENTS.no-memory.md` | Remove file-memory workaround advice |
| `docs/book/src/channels/acp.md` | New "Memory" section |

## Not in scope

- Chat session memory behavior (unchanged)
- Channel-driven sessions (unchanged)
- Cron/daemon sessions (unchanged)
- ACP session deletion API (already exists via session lifecycle)
- `hardware_memory_map` / `hardware_memory_read` tools — these are hardware
  introspection, not agent persistent memory; they stay
