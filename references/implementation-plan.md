# SQL Optimizer CLI — Project Context & Implementation Plan (v2)

**Purpose:** supersedes `implementation-plan.md` v1. Same intent — full conceptual context,
current state, and a phased, checkpointed plan, not a spec to blindly execute — but expanded to
cover workload regression tracking, safe fix generation, ORM-aware analysis, project-wide
scanning, PR/CI annotations, cost-aware analytics framing, richer index/partitioning guidance,
query normalization/dedup, real environment stats, and broader security posture checks. Where a
design decision is open, it's flagged as such.

**What changed vs v1:** platform scope narrowed to Unix (Linux/macOS) only; added a schema tree
diagram command to Phase 1; added a foundational normalization/fingerprinting phase (1.5) that
several later phases depend on; split "real environment stats" into its own phase feeding
everything downstream; substantially expanded workload regression detection into a real
time-series capability instead of a single baseline rerun; added three new major phases
(project-wide scanning, ORM-aware analysis, CI/PR annotations) that didn't exist before.

---

## 1. What this project is

A Unix-native (Linux/macOS only — Windows is explicitly out of scope) CLI that connects to a
real PostgreSQL, MySQL, or SQLite database (plus Supabase/Neon as Postgres-compatible targets),
analyzes SQL queries — handwritten, ORM-generated, or extracted from a whole project — against
real schema, real index, and real runtime stats, and gives prioritized, confidence-labeled
recommendations: security issues, missing/composite/covering index opportunities, partitioning
opportunities, query rewrites with previews, cost-aware analytics framing, and regressions in
query performance over time. It also renders a live schema as a tree diagram on request. It is
**not** a GUI, **not** an ORM, **not** a continuous monitoring daemon, and explicitly **not** a
fit for non-SQL databases (Convex remains out of scope — see §2.4).

### 1.1 The core differentiator

Nobody in this space bundles all of the following:

1. **Static analysis** — catch bad patterns before a query ever touches a database.
2. **Live schema/plan/stats awareness** — verify against real indexes, real query cost, and real
   runtime statistics (`pg_stat_statements`, `performance_schema`), not guesses.
3. **Time-aware workload tracking** — the same query, fingerprinted and watched across runs, so
   regressions (slower, lost an index, scanning more rows) are caught as they happen, not
   rediscovered from scratch each time.
4. **Project-wide reach** — analyze a folder of `.sql` files, dbt models, migrations, app source
   containing embedded SQL, or slow-query logs, not just one query pasted into a terminal.
5. **Serving both OLTP (app/backend) and analytics (data engineering) audiences explicitly**, via
   a profile concept, rather than picking one lane.

If schema/plan/stats awareness never gets built, this is "yet another SQL linter." If workload
tracking never gets built, it's "a linter that forgets everything between runs." Both are treated
as foundational, not optional.

### 1.2 The two audiences (do not conflate them)

| | Backend / app engineer | Data engineer |
|---|---|---|
| Query origin | Hand-written or ORM-generated, runs constantly | Written once, runs on a schedule/pipeline |
| Failure mode | Slow page load, N+1 from an ORM, missing index on a hot path | Job takes hours instead of minutes, blows warehouse cost budget |
| "Optimize" means | Milliseconds, index usage, lock contention | Join order, partition pruning, bytes scanned, dollar cost |
| Where they run this | Ad hoc while debugging, in a PR check, or against ORM-generated queries | Before deploying a dbt model / scheduled job, or scanning a whole models/ folder |

A `profile` parameter (`oltp` | `analytics`) is threaded through the whole recommendation
pipeline from day one (unchanged from v1) — retrofitting it later means touching every detector
twice.

### 1.3 The two usage modes (do not conflate these either)

Unchanged from v1: manual/interactive vs. CI/pipeline are different needs, not different tools.
**`analyze` and `batch` must never prompt for input, confirmation, or "did you mean...?"** A
prompt that's harmless for a human hangs a CI job until timeout. This rule now also applies to the
new project-wide scan command and any annotation-emitting mode.

### 1.4 Platform scope, decided (updated)

- **Unix only (Linux, macOS).** Windows support is dropped entirely — not deferred, dropped. It
  buys nothing but packaging/CI matrix size and path-handling edge cases (`~` expansion,
  `crossterm`/TTY quirks) for a side project. `scripts/install.sh` remains the only install path.
- **PostgreSQL, MySQL, SQLite, Supabase, Neon** — unchanged from v1 (see v1 §1.4 reasoning).
- **Convex — still explicitly out of scope.** Unchanged reasoning from v1.

### 1.5 New capability areas introduced in this revision

- **Schema tree diagram** — render the introspected schema (tables → columns → indexes, and once
  FK introspection is added, relationships) as a terminal tree via a dedicated subcommand.
- **Query normalization & fingerprinting** — canonicalize queries (strip literals, normalize
  whitespace/casing) into a stable identity so "the same query" can be recognized across runs,
  across a project, and across noisy real-world variation.
- **Workload regression tracking** — persist fingerprinted run history locally and flag when a
  known query gets slower, loses an index, or starts scanning more rows, especially after a
  schema or data change.
- **Real environment stats** — pull `pg_stat_statements` (Postgres), `performance_schema` (MySQL),
  and table cardinality so recommendations can be ranked by actual measured impact, not just
  static heuristics.
- **Project-wide scanning** — analyze a folder: raw `.sql` files, migration files, dbt models,
  application source (regex/heuristic extraction of embedded SQL), and slow-query logs.
- **ORM/framework-aware analysis** — recognize shapes typical of Rails/ActiveRecord, Django,
  Prisma, and Knex, since many expensive queries are generated indirectly, not handwritten.
- **CI/PR annotations** — emit SARIF, GitHub Actions inline annotations, or GitLab Code Quality
  JSON so findings land directly in code review, not just exit codes.
- **Deeper index/partitioning guidance** — composite indexes, covering indexes, partitioning
  opportunities, and the negative case ("an index would not help here and here's why").
- **Security posture beyond injection strings** — privilege overreach (does the connected role
  have more access than the query needs), on top of existing column-sensitivity heuristics.

---

## 2. Current state (ground truth, unchanged from v1 — verified against the actual repo)

**What works:** CLI parsing (`clap`) with `analyze`/`interactive`/`batch`; `sqlparser`-based AST
parsing, dialect-aware; two shallow detectors (`SELECT *` without `WHERE`, `IN` subquery →
suggest JOIN); substring-match security scan; text/JSON/YAML output.

**What does not work, despite appearances:** `PostgresConnector`/`MySqlConnector` don't actually
connect; `--explain` is a static placeholder; no schema introspection exists yet, so the README's
sample "Missing Index" output isn't producible; `patterns/*.rs`, `security/*.rs` (beyond the
inline stub), `rewriting/rewriter.rs`, `core/optimizer.rs`, `utils/*.rs` are one-line stubs; both
test files are empty; `docs/architecture.md` describes intended end-state, not current reality;
housekeeping bugs (UTF-16 files, `Makefile` binary-name mismatch) remain unfixed.

**Implication, unchanged:** Phase 1 (real DB connections + schema introspection) is the
prerequisite almost everything else — including every new capability in this revision — depends
on being truthful rather than decorative.

---

## 3. Design decisions already made (do not re-litigate these mid-build)

Carried over from v1 (unchanged): one engine/two consumption modes; `analyze` serves both
audiences; `batch` is the CI workhorse; `interactive` stays purely human-facing; `--ci` is sugar
for a flag bundle; three-level config precedence; baseline diffing; distinct exit codes;
confidence labeling on every recommendation; CLI-only, no GUI; feature priority order (fix
suggestions → cost-aware analytics → workload regression → migration/rewrite automation); input
sources include SQL embedded in app code; long-term goal includes migration/patch generation.

**New decisions for this revision:**

11. **The tool is stateless by default; history is opt-in.** A single `analyze` call never writes
    anything to disk unless the user opts in (e.g., a `.sql-optimizer/` state directory exists in
    the project, or `--track` is passed). This preserves the "quick, no side effects" property of
    the core command while enabling regression tracking for users who want it.
12. **Local state store is a single local SQLite file** (`.sql-optimizer/history.sqlite`, gitignored
    by default), holding: query fingerprints, per-run stats snapshots, and baseline references. No
    external service, no daemon — reading and writing it is just another file operation the CLI
    performs synchronously.
13. **Query fingerprinting prefers the database's own notion of query identity where one exists**
    (Postgres's `queryid` from `pg_stat_statements`), and falls back to an AST-based canonicalization
    (strip literals, normalize whitespace/case/ordering-insensitive clauses) elsewhere (MySQL,
    SQLite, or Postgres without the extension enabled). Both paths must produce the same fingerprint
    for the same logical query so history stays continuous even if the extension gets enabled later.
14. **Real environment stats are read-only, snapshot-based, and must degrade gracefully.**
    `pg_stat_statements` may not be installed; `performance_schema` may not be enabled; the
    connecting role may lack `pg_read_all_stats`/`PROCESS` privilege. Every consumer of these stats
    must have a defined fallback (usually: fall back to static/AST-based confidence) rather than
    fail the whole command.
15. **Project-wide scanning uses a common `SourceExtractor` abstraction** — one trait, multiple
    implementations (raw `.sql`, dbt models with best-effort Jinja `{{ ref() }}`/`{{ source() }}`
    stripping, app source via heuristic string-literal extraction, slow-query/general-query log
    parsing) — each yielding `(query_text, origin_location)` pairs into the same analysis pipeline
    used by `analyze`/`batch`. No new analysis logic is needed per source type, only extraction.
16. **ORM signature detection is explicitly heuristic and labeled as such.** It never claims
    schema- or plan-verified confidence on its own — it can only say "this looks like it came from
    ActiveRecord/Prisma/Knex/Django and matches a known anti-pattern shape," which is a distinct,
    lower confidence tier from the existing syntactic/schema/plan tiers.
17. **Privilege overreach checks require introspecting grants**
    (`information_schema.role_table_grants`, `pg_roles` on Postgres; grant tables on MySQL, which
    often themselves require elevated privilege to read) and must degrade to "not checked" rather
    than fail when the connecting role can't see its own grants.
18. **Annotation output is a separate concern from human-readable output**, selected via
    `--annotate github|gitlab|sarif`, and is additive: it never replaces exit codes or the
    existing text/JSON/YAML formatters, only supplements them for CI consumers that render
    inline PR feedback.

---

## 4. Feature request → plan mapping (explicit confirmation)

| Requested feature | Fits existing plan? | Disposition |
|---|---|---|
| Workload-aware regression detection (fingerprint over time, schema/data-change correlation) | Partially — old Phase 3.7 was a single baseline rerun | Rewritten as Phase 3.8, now depends on Phase 1.5 (fingerprinting) and Phase 3.7 (real stats) |
| Safe fix generation with diff preview + confidence | Yes | Phase 3.5, extended to include migration DDL |
| ORM/framework-aware analysis | New | Phase 6 |
| Project-wide scanning (folders, migrations, dbt, app logs) | New | Phase 5 |
| PR/CI annotations (GitHub/GitLab) | Partially — exit codes/`--ci` existed, annotation formats didn't | Phase 7 (was Phase 5 in v1), extended |
| Cost/dollar impact framing for analytics | Yes | Phase 3.6, extended with $-estimate framing |
| Composite/covering indexes, partitioning, "index wouldn't help" | Partially — old missing-index detector was narrower | Expanded within Phase 3 |
| Query normalization & deduplication | New, and foundational | Phase 1.5 |
| Real environment awareness (`pg_stat_statements`, `performance_schema`, cardinality) | New | Phase 3.7 |
| Security beyond injection strings (privilege overreach) | Partially — sensitive-column heuristics existed | Expanded within Phase 3 |
| *(carried from prior discussion)* Schema tree diagram | New | Folded into Phase 1 |
| *(carried from prior discussion)* Plain-English EXPLAIN walk | New, small | Folded into Phase 1 |
| *(carried from prior discussion)* Schema drift detection | New, small | Folded into Phase 7 (reuses baseline machinery) |

---

## 5. Phased implementation plan with checkpoints

Time estimates assume one focused contributor (human or agent) at roughly full-time pace. Each
checkpoint has a concrete "done" test — if you can't demonstrate it, the phase isn't done.

### Phase 0 — Foundation hygiene
**Target: 2–3 days**

- Fix `Makefile`/`install.sh` binary name mismatch. Convert UTF-16 files to UTF-8.
- Write real content into the two empty test files: at least one real unit test per existing
  detector, one real integration test running `analyze` end-to-end.
- Formally drop Windows from scope: remove any Windows-specific packaging assumptions, note it in
  the README's Requirements section.
- **Checkpoint test:** `make check` (fmt + clippy + test) runs clean on Linux and macOS, and the
  test suite actually fails if you deliberately break a detector.

### Phase 1 — Real database layer + schema introspection (blocking prerequisite)
**Target: 1.5–2.5 weeks** (extended slightly for the tree command and EXPLAIN narration)

- Real `tokio-postgres` and `mysql_async` connections; real error paths, not silent success.
- SQLite support (already largely in place per the repo — verify against this phase's bar).
- `SchemaIntrospector`: tables, columns, indexes (already largely built for Postgres/MySQL/SQLite
  per current source) — extend to also pull foreign keys, needed for the tree diagram's
  relationship edges.
- Real `EXPLAIN` execution parsed into one common internal plan representation.
- TLS/`sslmode=require` support.
- **New: `schema` subcommand** — renders the introspected `SchemaSnapshot` as a Unix-terminal tree
  (tables → columns → indexes →, once FKs exist, relationships to other tables). Pure read of data
  Phase 1 already produces; no new DB round-trips beyond introspection.
- **New: plain-English EXPLAIN walk** — a short prose summary of the parsed `QueryPlanNode` tree
  ("sequential scan on `orders`, ~40k rows, no index used") alongside the existing raw plan output.
- **Checkpoint test:** against a real local Postgres, MySQL, and SQLite instance, the tool can (a)
  connect, (b) list real indexes and FKs, (c) run EXPLAIN and get a parsed plan object, (d) render
  `schema` as a tree, (e) print a plain-English one-line plan summary.

### Phase 1.5 — Query normalization & fingerprinting engine
**Target: 1–1.5 weeks** (new phase; sequenced here because Phases 3.8, 5, and 6's dedup all need it)

- Canonicalize a query: strip literal values, normalize whitespace/casing, normalize
  order-insensitive clause formatting, produce a stable fingerprint (hash).
- On Postgres, prefer the `queryid` exposed by `pg_stat_statements` when available; otherwise use
  the AST-based canonicalization. Ensure both paths converge on the same fingerprint for the same
  logical query.
- Build a dedup/grouping utility: given a set of queries (from `batch` or project-wide scanning),
  group by fingerprint and surface "top offenders" — same fingerprint, most occurrences or worst
  measured cost — instead of a thousand noisy literal-varying duplicates.
- **Checkpoint test:** feeding the same query with different literal values (`WHERE id = 1` vs
  `WHERE id = 2`) produces the same fingerprint; feeding genuinely different queries produces
  different fingerprints; a batch of 50 queries with 5 distinct shapes groups into 5 buckets.

### Phase 2 — Cloud Postgres compatibility (Supabase / Neon)
**Target: 3–5 days** — unchanged from v1.

- **Checkpoint test:** unchanged — `analyze` runs end-to-end against a live Supabase and Neon URL.

### Phase 3 — Real detectors, replacing the stubs
**Target: 3–4 weeks** (extended from v1 to cover composite/covering indexes, partitioning, and
privilege overreach), ordered cheapest-to-verify first:

1. `patterns/missing_index.rs` — cross-reference WHERE/JOIN/ORDER BY columns against introspected
   indexes. **Expanded scope:** also suggest composite indexes (multi-column WHERE/JOIN patterns),
   covering indexes (when a query only needs columns already in a candidate index's key + include
   list), partitioning opportunities (large tables filtered heavily on a single column, e.g. a
   timestamp — Postgres declarative partitioning specifically), and the negative case: explicitly
   state when an index would *not* help (e.g. low-selectivity column, small table) so the tool
   doesn't over-recommend.
2. `patterns/cartesian_product.rs` — unchanged from v1.
3. `patterns/inefficient_join.rs` — unchanged from v1, sequenced after plan parsing is solid.
4. `security/sensitive_data.rs` — column-name heuristics cross-referenced against real schema.
   **Expanded scope:** privilege overreach — introspect the connected role's grants and flag when
   it holds broader privileges (e.g. `DELETE`/`DROP`) than the queries it's actually issuing need,
   degrading to "not checked" when grant visibility isn't available (per design decision 17).
5. `security/injection.rs` — replace substring matching with parameterization-awareness (bound
   parameter vs. string-concatenated literal).
6. `rewriting/rewriter.rs` — deliberately last, unchanged reasoning from v1.

Every recommendation carries a confidence label (syntactic guess / schema-verified /
plan-verified / ORM-heuristic — the last added in Phase 6).

- **Checkpoint test:** for each detector, a paired "should trigger"/"should not trigger" fixture
  run against a real database with real schema, including at least one composite-index case, one
  partitioning case, one "index wouldn't help" case, and one privilege-overreach case.

### Phase 3.5 — Fix suggestions and rewrite previews
**Target: 1.5–2.5 weeks** (extended slightly for migration DDL)

- Turn high-confidence detector output into actionable fixes, not just diagnostics.
- Diff-style before/after preview for query rewrites.
- **New:** for index/partitioning recommendations, generate the actual `CREATE INDEX`/partitioning
  migration DDL as part of the preview, not just prose describing the fix — this is the
  concretely useful artifact a backend engineer copies into a migration file.
- **Checkpoint test:** a query with a known issue produces a concrete fix; an index recommendation
  produces runnable DDL; the tool shows before/after without mutating the original query.

### Phase 3.6 — Cost-aware analytics recommendations
**Target: 1–2 weeks** (extended slightly for dollar framing)

- Scan volume, join cost, partition pruning, aggregation cost, rough operational impact.
- **New:** an explicit approximate-dollar-impact framing where feasible (e.g., rows/bytes scanned
  translated into a rough compute-cost estimate for common warehouse pricing models), clearly
  labeled as an estimate, not a bill.
- **Checkpoint test:** the same analytics query yields recommendations that name scan/cost impact
  and, where applicable, a labeled rough dollar estimate.

### Phase 3.7 — Real environment awareness / DB health snapshot
**Target: 1–1.5 weeks** (new phase, feeds ranking into 3.6, 3.8, 5, and 6)

- Read `pg_stat_statements` (Postgres) and `performance_schema`/slow query log (MySQL) where
  available; read table cardinality (`pg_stat_user_tables`, `information_schema.tables` row
  estimates) for both.
- Expose this as a `health`/`stats` subcommand: a point-in-time snapshot, explicitly not a
  monitoring daemon (per prior discussion).
- Feed real measured cost into ranking: when stats are available, "top offenders" and
  recommendation ordering use actual observed cost instead of only static heuristics.
- Degrade gracefully and say so explicitly when the extension/permission isn't available.
- **Checkpoint test:** against a Postgres instance with `pg_stat_statements` enabled, `health`
  returns real top queries by total time; against one without it, the command still succeeds and
  clearly states the limitation instead of failing.

### Phase 3.8 — Workload regression detection over time
**Target: 1.5–2 weeks** (substantially expanded from v1's single-baseline-rerun version)

- Persist fingerprinted run history to the local state store (design decision 12), keyed by
  fingerprint from Phase 1.5.
- On each tracked run, compare the current stats snapshot (Phase 3.7, where available) or plan
  shape (Phase 1) against history for the same fingerprint; flag: got slower, lost an index that
  was previously used, started scanning materially more rows.
- Best-effort correlation with schema changes: if a regression appears right after an index was
  dropped or a column type changed (visible via schema snapshot diffing), say so explicitly.
- Opt-in via `--track` or presence of `.sql-optimizer/` (design decision 11).
- **Checkpoint test:** run a query against a schema with an index, drop the index, rerun — the
  tool reports a regression tied to that fingerprint and names the lost index as the likely cause.

### Phase 4 — Profile-aware recommendations (OLTP vs analytics)
**Target: 1 week** — unchanged from v1.

### Phase 5 — Project-wide scanning & multi-source ingestion
**Target: 2–3 weeks** (new major phase)

- Implement the `SourceExtractor` abstraction (design decision 15).
- Extractors: raw `.sql` files/folders; SQL migration files; dbt models (best-effort Jinja
  `{{ ref() }}`/`{{ source() }}` stripping — document clearly what isn't fully resolved, e.g.
  macros that require the dbt compiler); application source files (heuristic extraction of
  string literals that look like SQL — language-agnostic regex pass, not a real parser for
  Ruby/Python/JS); slow-query and general-query log files (Postgres and MySQL log formats).
- Route every extracted query through the existing `analyze` pipeline and the Phase 1.5 dedup
  engine, so a project scan surfaces "top offenders" across the whole codebase, not a flat list.
- New `scan <path>` subcommand; must never prompt (per §1.3) and must produce the same
  machine-readable output shapes as `batch`.
- **Checkpoint test:** pointed at a small fixture project (mixed `.sql` files, one dbt model, one
  app-source file with an embedded query string, one slow-query log sample), `scan` extracts all
  of them, dedups equivalent shapes, and reports top offenders with origin file/line.

### Phase 6 — ORM & framework-aware analysis
**Target: 1.5–2.5 weeks** (new major phase, builds on Phase 5 for source-linked mode)

- Maintain a small library of shape-based signatures per ORM/framework (Rails/ActiveRecord,
  Django, Prisma, Knex) — recognizable column-ordering/aliasing/quoting conventions and known
  anti-pattern shapes (e.g. missing eager-loading causing N+1).
- Two modes: **shape-only** (works on a single query with no source context — lower value but
  works standalone) and **source-linked** (via Phase 5's extractor, ties a flagged query back to
  the originating ORM call site in app source).
- Every ORM-derived finding is labeled with the new `orm-heuristic` confidence tier (design
  decision 16), never elevated to schema/plan-verified on its own.
- **Checkpoint test:** a known ActiveRecord N+1 shape and a known Prisma over-fetch shape are both
  correctly identified and labeled `orm-heuristic`; a handwritten query with no ORM markers is not
  misclassified as ORM-generated.

### Phase 7 — CI/pipeline-aware CLI surface & PR/code-review annotations
**Target: 2–2.5 weeks** (was Phase 5 in v1, expanded with annotation formats)

- `--fail-on <severity>`, distinct exit codes (clean / warnings-only / blocking-found /
  tool-error), `--baseline <file>`, `--ci` convenience flag, `.sql-optimizer.toml` config
  layering — all unchanged from v1.
- **New: `--annotate github|gitlab|sarif`** — emit GitHub Actions inline workflow-command
  annotations, GitLab Code Quality JSON, or SARIF (for GitHub code scanning), additive to existing
  output formats (design decision 18).
- **New: schema drift detection** — reusing the baseline-diffing machinery already being built
  here, snapshot the schema tree (Phase 1) and diff against a prior snapshot to flag things like
  "index `idx_users_email` was dropped since last run."
- Harden `analyze`/`batch`/`scan` to structurally guarantee no stdin prompts under any code path.
- **Checkpoint test:** a real GitHub Actions workflow in this repo runs `analyze --ci --annotate
  github` against a test query set, produces inline annotations on an intentionally-bad query,
  fails the job on it, and passes cleanly on a good one — committed and demonstrably green/red.

### Phase 8 — Polish, documentation, and packaging
**Target: 1–1.5 weeks** (was Phase 6 in v1)

- Update README to reflect actual capability, including the Unix-only requirement.
- Document the config file schema, all CI flags, annotation formats, and the opt-in local state
  store, with copy-pasteable GitHub Actions/GitLab CI snippets.
- Verify Linux and macOS builds succeed via the Makefile; drop any cross-compilation targeting
  Windows.
- Tag a 1.0 release only once Phases 1–7's checkpoint tests all still pass together in one run.

---

## 6. Running total & sequencing notes

Total estimated span: **~16–20 weeks** of focused work end-to-end — longer than v1's 7–9 weeks,
because this revision adds three genuinely new major phases (project-wide scanning, ORM-aware
analysis, expanded regression tracking) rather than just extending existing ones.

**Sequencing dependencies to respect:**

- Phase 1.5 (fingerprinting) must land before Phase 3.8 (regression tracking), Phase 5 (project
  scan dedup), and Phase 6 (ORM dedup) — all three consume it.
- Phase 3.7 (real environment stats) should land before Phase 3.8, since regression detection is
  far more precise with real observed stats than with plan-shape comparison alone, though plan-
  shape comparison remains a valid fallback when stats aren't available (design decision 14).
- Phase 5 (project scanning) should land before Phase 6's source-linked mode, though Phase 6's
  shape-only mode can ship independently and earlier if desired.
- Phase 7's annotation work depends only on Phase 3's detectors and the existing baseline/exit-code
  machinery — it does not need Phases 5 or 6 to be useful, so it can be pulled forward if CI
  integration is the more urgent priority than project-wide scanning or ORM-awareness.

**Do not reorder Phase 1 or Phase 1.5 behind feature work that looks more exciting.** Every
downstream phase in this revision — including the three new major ones — implicitly depends on
schema introspection, real EXPLAIN parsing, and stable query fingerprinting being real. If time
pressure forces a cut, cut breadth (fewer extractors in Phase 5, fewer ORM signatures in Phase 6,
fewer annotation formats in Phase 7) before cutting Phase 1/1.5's depth.
