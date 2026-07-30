# Auditing progress reports against ground truth

This project has already had at least one case where an AI coding agent's progress report
misrepresented the actual state of the repo in a structurally significant way — specifically,
describing detection logic as part of the modular `patterns/*.rs`/`security/*.rs` architecture
when the logic was actually hardcoded inline in `core/analyzer.rs`, with the modular files
remaining empty stubs with no callers. The Phase 0 claim "the test suite would fail if a detector
were broken" was true only for the inline logic, not for the modular system later phases depend
on. Treat this as the default failure mode to check for, not an edge case.

**Core rule: a progress report is a claim, not a source of truth. The repo's actual files are the
source of truth. Always verify the former against the latter before accepting it.**

## When to run this audit

- Whenever an agent (yourself in a prior session, another AI agent, or a human collaborator's
  summary) reports a phase, checkpoint, or feature as "done," "working," or "implemented."
- Before starting new work on top of a claimed foundation — if Phase 2 work is about to build on
  a claimed-complete Phase 1, verify Phase 1 first.
- Periodically during long autonomous sessions, even without an explicit request, if you notice
  you're about to build on top of an assumption you haven't personally verified in this session.

## The audit procedure

1. **Get the real file tree first, before reading the report closely.**
   ```bash
   find . -type f -not -path './.git/*' | sort
   ```
   Compare this against what the report implies exists. A report mentioning a module by name does
   not mean the module has real content — check.

2. **For every module or file the report claims is "implemented" or "working," open it and read
   it.** Don't infer content from the filename or from the report's description. Specifically look
   for:
   - One-line placeholder stubs (`// Placeholder implementation` is the marker historically used
     in this repo).
   - Functions that are no-ops disguised as real implementations — e.g. a method that returns
     `Self::new()` or an unchanged value instead of doing the described work (this exact pattern
     was found in `SqlAnalyzer::with_database()`).
   - Empty test files, or test files containing only a byte-order-mark / whitespace.

3. **Distinguish "logic exists" from "the modular system is wired up."** This is the single most
   important and most easily missed distinction in this codebase. Concretely:
   - Check whether `src/patterns/mod.rs` and `src/security/mod.rs` files are declared in
     `src/lib.rs`/`src/core/mod.rs` — module declaration existing is not the same as the module
     being called.
   - Grep for actual call sites:
     ```bash
     grep -rn "patterns::" src/core/ src/cli/
     grep -rn "security::" src/core/ src/cli/
     ```
     If a pattern/security submodule has zero call sites outside its own file, it is not wired up,
     regardless of what a report says about it.
   - Compare this against where the equivalent logic actually lives — historically, detection logic
     for `SELECT *`, N+1 subqueries, and security keyword scanning has lived inline inside
     `core/analyzer.rs` methods rather than in the dedicated pattern/security files built for it.

4. **Verify claimed checkpoint tests actually pass, and actually test what they claim to.** Don't
   accept "tests pass" — run them, and separately check that a test asserting on a detector would
   actually fail if that detector were broken (deliberately break one and confirm, if the stakes
   warrant it).

5. **Verify referenced build/tooling artifacts exist before trusting claims about them.** E.g., if
   a report references a `make check` checkpoint, confirm a `Makefile` with a `check` target
   actually exists in the repo before treating the checkpoint as meaningful.

6. **Check Cargo.toml/dependency claims directly rather than trusting a description of them.** A
   report describing a Cargo feature as "newly added" or "still needed" should be checked against
   the actual `Cargo.toml` — dependencies already present get misdescribed as missing at least as
   often as missing ones get overlooked.

## Reporting the audit

When summarizing an audit back to Anthony, structure it as:

- What's legitimately real and working (be specific — file, function, what it actually does).
- What's a stub, no-op, or otherwise not what it was described as (be specific, cite the file).
- The single most structurally important gap, called out explicitly and separately from the rest
  — in past audits this has been the inline-vs-modular architecture gap, since it determines
  whether later phases have real groundwork or not.
- Do not propose code fixes unsolicited as part of the audit — this is a review/conceptual task
  by default (see conventions.md). Wait to be asked before patching anything found.

## Automatable parts of this checklist

`scripts/audit_repo.sh` in this skill automates steps 1–3 (file tree, stub detection, module
wiring check) so they run consistently instead of being re-derived by eye each time. Run it first,
then read the flagged files yourself — the script surfaces suspects, it doesn't replace reading
the actual code.