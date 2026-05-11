# v0.8.0 Excision Pass — Incident Log

Working audit trail for the v0.8.0 excision pass. Each entry records a deletion candidate where the decision wasn't pure-delete: the surrounding test was real, the call-site couldn't be reached safely, or the suppression turned out to mark live code.

Format: site, decision (deleted / kept / kept-with-narrow), reason.

## Phase 1 — Orphaned files

- `v3.toml` (785 lines, repo root) — **deleted**. Zero references in code, docs, tests, scripts, .gitignore, CI. Residue from the scrapped `zeroclaw config generate` (commit 73f906474).
- `release-notes-notes.md` (32 lines, repo root) — **deleted**. Scratch TODOs accidentally committed; bullets belong in the runbook PR.

## Phase 2 — `#[allow(dead_code)]` sweep

(populated as the sweep runs)

## Phase 3 — Stale comment refs (PR / issue / phase numbers)

(populated)

## Phase 4 — Stale `#[serde(alias)]`

(populated)

## Phase 5 — `channels_except_webhook` + `channels` hand-rolled lists

(populated)

## Phase 6 — FeishuConfig folded into LarkConfig

(populated)
