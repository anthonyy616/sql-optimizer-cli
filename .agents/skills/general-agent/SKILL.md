---
name: sql-optimizer-cli-dev
description: Use this skill for ANY work on the sql-optimizer-cli Rust project (repo anthonyy616/sql-optimizer-cli) — implementing a phase from the roadmap, adding a detector/connector/CLI flag, reviewing or auditing an agent's progress report or "done" claim, deciding what to build next, or resuming work after a break. Trigger it even if the user just says something like "continue working on the SQL tool," "what's next," "check what the last agent actually did," or pastes a progress summary for this project — don't wait for an explicit request to "use the skill." This skill carries the full project vision, locked-in design decisions, phase-by-phase plan, repo-specific gotchas, and a mandatory ground-truth audit step before any claim of "done" is trusted.
---

# sql-optimizer-cli development

This skill exists because this project has already been burned once by an agent (possibly a prior
instance of yourself) describing the repo's state inaccurately in a way that would have quietly
undermined later phases. Treat verification as part of the job, not an optional extra step.

## Orientation — read this first, every session

1. **The plan lives in `references/implementation-plan.md`.** It is the source of truth for
   vision, audience, design decisions (§3), the phase list with checkpoint tests (§5), and
   sequencing dependencies (§6). Read the relevant phase section before writing any code — don't
   reconstruct the plan from memory or from what a past summary implied.
2. **Repo/tooling specifics live in `references/conventions.md`.** Read this before touching the
   repo at all — it covers WSL/Unix-only scope, how to reliably read the repo (`git clone`, not
   `web_fetch`), the UTF-16LE encoding gotcha, and the design rules that are easy to accidentally
   violate mid-implementation (stdin-prompt bans, confidence labeling, statelessness-by-default,
   etc.).
3. **Never trust a "this phase is done" claim — verify it.** Full methodology in
   `references/audit-checklist.md`; automated first pass via `scripts/audit_repo.sh`. This applies
   whether the claim comes from a prior AI agent's summary, your own earlier turn in a long
   session, or a human's recollection.

## Standard workflow for a work session

1. **Get the real file tree and run the audit script before doing anything else**, even if nobody
   asked for an audit. This is cheap and prevents building on a false foundation.
   ```bash
   ./scripts/audit_repo.sh <path-to-cloned-repo-or-git-url> [branch]
   ```
   Read the output. Cross-check anything it flags against the actual file content yourself — the
   script surfaces suspects (stubs, unwired modules, encoding issues, binary-name mismatches), it
   does not replace reading the code.
2. **Identify the current phase** from `references/implementation-plan.md` §5, based on what the
   audit actually shows exists and works — not based on what was previously claimed to exist.
   If the audit contradicts an earlier "phase N complete" claim, say so explicitly and treat phase
   N as incomplete until its checkpoint test is demonstrated for real.
3. **Before implementing, re-check the design rules in `conventions.md`** relevant to what's being
   built (e.g., adding a new CLI command → check the no-stdin-prompt rule and the `--ci`-mode
   implications; adding a detector → check the confidence-label and `profile`-parameter rules;
   adding a connector → check the "Supabase/Neon are not new connectors" rule).
4. **Implement to the phase's stated checkpoint test, and actually demonstrate it** — run it, show
   the output, don't just assert it would pass. If a checkpoint test can't be demonstrated in the
   current environment (e.g., no live Supabase project available), say so plainly rather than
   marking the phase done anyway.
5. **When reporting back, separate three things clearly**: what was actually implemented and
   verified this session, what remains for this phase, and any discrepancy found between a prior
   claim and actual repo state. Don't blend "what I did" with "what the repo already claimed to
   have" — that blending is exactly how the original audit gap happened.

## When the task is review/audit rather than implementation

If asked to review a plan, review someone else's (or your own prior) progress claim, or decide
what to prioritize next — **this is a conceptual review task, not an invitation to start editing
code.** Give a direct, specific assessment (see `references/audit-checklist.md` for the reporting
structure) and wait to be asked before making changes. This matches how work on this project is
generally expected to go — implementation happens when explicitly requested, review happens on
its own first.

## When the task is clearly implementation

Proceed directly: read the relevant phase, check conventions, implement, verify against the
checkpoint test, report clearly. Don't ask for permission to write code once implementation has
been explicitly requested — that's already been established as the mode for that turn.

## Quick reference index

| Need | Go to |
|---|---|
| Full plan, phases, checkpoint tests, design decisions | `references/implementation-plan.md` |
| Repo gotchas, environment facts, hard constraints | `references/conventions.md` |
| How to verify a "done" claim, what to check, how to report it | `references/audit-checklist.md` |
| Automated stub/wiring/encoding/binary-name check | `scripts/audit_repo.sh` |

## A note on scope drift

This project's plan (`implementation-plan.md`) is intentionally large — 8+ phases spanning DB
connectors, detectors, workload regression tracking, project-wide scanning, ORM-awareness, and CI
integration. **Do not silently reorder or skip phases because later work looks more interesting.**
§6 of the plan states explicit sequencing dependencies (e.g., Phase 1.5's fingerprinting must
exist before Phase 3.8's regression tracking). If you believe a reorder is genuinely warranted,
say so explicitly and explain why, rather than just building out of order without comment.