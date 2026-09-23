# SQL Optimizer CLI

A Unix-native command-line tool that analyzes SQL queries against a **real** database — real schema, real indexes, real runtime stats — and returns prioritized, confidence-labeled recommendations: security issues, missing/composite/covering index opportunities (with runnable `CREATE INDEX` DDL), partitioning candidates, query rewrites with before/after previews, cost-aware analytics framing, and workload regressions tracked over time.

Supported targets: **PostgreSQL**, **MySQL**, **SQLite** (plus Supabase/Neon as Postgres-compatible). Linux and macOS only.

## Features

- **Static analysis** — N+1 shapes, Cartesian products, inefficient JOINs, `SELECT *`, injection-prone string concatenation
- **Live schema awareness** — cross-references WHERE/JOIN/ORDER BY columns against introspected indexes; knows when an index *wouldn't* help
- **Real EXPLAIN parsing** — one internal plan representation across dialects, plus a plain-English plan summary
- **Schema tree** — render tables → columns → indexes → foreign keys as a terminal tree
- **Query fingerprinting** — literal-stripped canonicalization so "the same query" is recognized across runs and files
- **Workload regression tracking** — opt-in local history (`.sql-optimizer/history.sqlite`); flags slower runs, lost indexes, more rows scanned
- **Health snapshot** — top queries by total time from `pg_stat_statements` / `performance_schema`, table cardinality; degrades gracefully when unavailable
- **Project-wide scanning** — raw `.sql`, migrations, dbt models, app source with embedded SQL, slow-query logs; deduplicated into "top offenders"
- **ORM-aware heuristics** — ActiveRecord / Django / Prisma / Knex shape detection, always labeled `orm-heuristic`
- **CI integration** — `--fail-on <severity>`, distinct exit codes, baselines, `--annotate github|gitlab|sarif`, `.sql-optimizer.toml` config
- **TUI dashboard** — full-screen interactive query workspace (`tui`) with Query/Analyze/Optimize/Schema/Health/History panels, saved connection profiles (keychain-backed secrets), and read-only query previews

## Installation

### From npm (recommended)

```bash
npm install -g sql-optimizer-cli
```

Requirements: Node.js 18+, Linux or macOS. No Rust toolchain needed — npm downloads the prebuilt binary for your platform automatically (`darwin-arm64`, `darwin-x64`, `linux-x64`, `linux-arm64` — all Linux builds are static musl binaries that run on any distro).

```bash
sql-optimizer-cli --version
sql-optimizer-cli schema --db sqlite::memory:
sql-optimizer-cli                # no args → TUI dashboard
```

Typing `sql-optimizer-cli` with **no arguments**, or with only flags (e.g. `sql-optimizer-cli --db postgresql://...`), launches the TUI. Subcommands (`analyze`, `scan`, …) work exactly as documented below — in all examples, substitute `sql-optimizer-cli` for `$BIN`.

The binary itself is also useful without Node afterwards; npm just delivers it.

### From Source
```bash
git clone https://github.com/anthonyy616/sql-optimizer-cli.git
cd sql-optimizer-cli
./scripts/install.sh
```

The install script builds the binary with cargo, puts `sql-optimizer-cli` on your PATH, and creates shortcut commands named `analyze`, `batch`, `interactive`, `schema`, `scan`, and `tui`. During local development use `cargo run --bin sql-optimizer-cli -- ...`.

### Run it without installing: `scripts/env.sh`

Don't want to install anything? Source one file to get `$BIN` (absolute path to
the debug binary) plus a `sqlopt` shortcut function in your current shell:

```bash
source scripts/env.sh          # builds the binary first if it's missing

$BIN schema --db sqlite::memory:
sqlopt analyze "SELECT 1" --db sqlite::memory:
sqlopt-rebuild                 # rebuild after code changes

# Make `sqlopt` available in every new shell (no rc-file editing):
source scripts/env.sh --install   # copies launcher to ~/.local/bin
```

To keep `$BIN`/`sqlopt` across sessions, add the source line to your shell rc:
`echo 'source "$HOME/path/to/sql-optimizer-cli/scripts/env.sh"' >> ~/.zshrc`
(requires bash or zsh; plain `sh`/dash does not support the functions).

The rest of this README uses `$BIN` in examples — with `env.sh` sourced, or
after `./scripts/install.sh`, just substitute your preferred way of invoking
the binary (`sql-optimizer-cli`, `sqlopt`, or `cargo run --bin
sql-optimizer-cli --`).

## Quick Start

Two connection styles:

1. Pass a full connection URL with `--db`.
2. Set `SQL_OPTIMIZER_DB_*` values in a `.env` file:

```bash
SQL_OPTIMIZER_DB_HOST=db.example.supabase.co
SQL_OPTIMIZER_DB_PORT=5432
SQL_OPTIMIZER_DB_USER=postgres
SQL_OPTIMIZER_DB_PASSWORD=your_password_here
SQL_OPTIMIZER_DB_NAME=postgres
SQL_OPTIMIZER_DB_SSLMODE=require
SQL_OPTIMIZER_DB_ACCEPT_INVALID_CERTS=false
```

For Supabase prefer the session pooler connection string, and add `--simple-mode` when connecting through PgBouncer-style poolers so the client avoids prepared statements.

```bash
# Smoke test: render the schema tree
$BIN schema --db "$SQL_OPTIMIZER_DB_URL"

# Analyze a query with plan + fix suggestions
$BIN analyze \
  "SELECT u.*, o.total FROM users u JOIN orders o ON u.id = o.user_id" \
  --db postgresql://user:pass@localhost:5432/mydb --explain

# Health snapshot (top queries by time, table sizes)
$BIN health --db "$SQL_OPTIMIZER_DB_URL"

# Scan a whole project: .sql files, dbt models, app source, slow logs
$BIN scan ./migrations --db "$SQL_OPTIMIZER_DB_URL" --output json

# Full-screen dashboard
$BIN tui --db "$SQL_OPTIMIZER_DB_URL"
```

## Command Reference

### Shared Flags

| Flag | Description |
| --- | --- |
| `-v`, `--verbose` | Print extra progress details before running the command. |
| `--profile <oltp\|analytics>` | Analysis profile threaded through every recommendation (global flag). Defaults to `oltp`. |
| `-d`, `--db <URL>` | Full database connection string; overrides individual parts. |
| `--db-host/--db-port/--db-user/--db-password/--db-name` | Build a connection string from parts. |
| `--db-sslmode <MODE>` | PostgreSQL SSL mode when building from parts. Defaults to `require`. |
| `--accept-invalid-certs` | Allow untrusted TLS certificate chains (local testing only). |
| `--simple-mode` | Avoid prepared statements — needed for PgBouncer transaction pooling. |
| `--connect-timeout <SECONDS>` | Connection timeout override. |

Every connection-related flag also reads its `SQL_OPTIMIZER_DB_*` environment equivalent.

### `analyze`

Analyze a single SQL query.

```bash
$BIN analyze <QUERY> [shared flags] [--explain] [--show-rows] [--row-limit N] \
  [--output text|json|yaml|markdown] [--track] [--schema-baseline <FILE>] [CI flags]
```

| Flag | Description |
| --- | --- |
| `--explain` | Include a parsed execution plan + plain-English summary. |
| `--show-rows` / `--row-limit <N>` | Preview matching rows for read-only SELECTs. |
| `-o`, `--output <FORMAT>` | `text`, `json`, `yaml`, or `markdown`. |
| `--track` | Record this run in the local state store for regression detection (also enabled implicitly by the presence of `.sql-optimizer/`). |
| `--schema-baseline <FILE>` | Diff live schema against a saved snapshot and report drift. |

### `batch`

Process multiple queries from a file.

```bash
$BIN batch --input queries.sql [--output-file FILE] [-o FORMAT] [shared flags] [CI flags]
```

With a non-text `--output` and no explicit output file, results are auto-written under `output/`.

### `scan`

Scan a file or directory: raw `.sql` files, migration files, dbt models (Jinja `{{ ref() }}` stripped best-effort), application source containing embedded SQL, and Postgres/MySQL log formats. Queries are fingerprinted and deduplicated; the report surfaces top offenders with origin file/line. Never prompts — safe for CI.

```bash
$BIN scan <PATH> [shared flags] [-o FORMAT] [--output-file FILE] [--schema-baseline FILE] [CI flags]
```

Exclusions come from `exclude` in `.sql-optimizer.toml`.

### `schema`

Introspect and print the schema as a tree (tables → columns → indexes → FKs).

```bash
$BIN schema [shared flags] [--save <FILE>]
```

`--save <FILE>` writes the snapshot JSON, which can later be passed to `--schema-baseline` for drift detection.

### `health`

Point-in-time DB health snapshot: top queries by total time (`pg_stat_statements` / `performance_schema`) and table cardinality. If the extension or privilege isn't available, the command still succeeds and says so explicitly — it is not a monitoring daemon.

```bash
$BIN health [shared flags]
```

### `interactive`

Classic line-based interactive session; keeps one connection open for the session.

```bash
$BIN interactive [shared flags] [--history ~/.sql-optimizer-history] [--show-rows] [-o FORMAT]
```

### `tui`

Full-screen terminal workspace with seven panels:

- **Connect** — pick a database visually: SQLite, PostgreSQL, MySQL, or Postgres-compatible clouds (Supabase, Neon). Press `Enter` on a provider to prefill its URL template, `a` to add any connection URL, `e` to edit, `d` to delete, `r` to retry, `x` to disconnect. Saved profiles persist user-globally (`~/.config/sql-optimizer/connections.json`, no passwords in the file — secrets go to the OS credential store, e.g. macOS Keychain) and can be switched at any time without restarting the TUI
- **Query** — type a read-only SELECT and press Enter to preview rows (writes/DDL are rejected at the connector boundary). `Ctrl+A` sends the query to Analyze. Every attempt lands in History
- **Analyze** — type a query, press Enter; results include recommendations, security findings, and plan summary. `Ctrl+P` carries the query back to the Query tab
- **Optimize** — the latest analysis's findings grouped and ranked: missing indexes, inefficient joins, cartesian products, N+1 patterns, rewrites — with confidence, verification status, and proposed SQL/diff as copy/preview only (nothing executes)
- **Schema** — press `s` to refresh the introspected tree
- **Health** — press `h` for a live stats snapshot
- **History** — selectable query runs with connection/status filters (`c`/`f`); `Enter` re-opens a run in Query, `Ctrl+A` analyzes it

The TUI **always launches**, even with no database connection or a failing one — a connection error (bad TLS cert, refused port, wrong credentials) lands you on the Connect tab with the failing URL pre-added to the catalog so you can retry, edit, or pick a different database. SQLite (in-memory) needs no server, so it's the fastest way in. All database work runs on a background worker, so the UI stays responsive (loading states per tab, no frozen keys) while queries/schema/health run, and switching connections never leaks stale results.

Keys: `Tab`/`←→` switch panels, `↑↓` select/scroll, `Enter` connect/analyze, `q`/`Esc` quit. Requires a real terminal (TTY).

```bash
$BIN tui                       # start unconnected; choose a DB in the Connect tab
$BIN tui --db "$SQL_OPTIMIZER_DB_URL"   # connect up-front; fall back to Connect tab on failure
```

## CI / Pipeline Usage

`analyze`, `batch`, and `scan` never prompt and share these flags:

| Flag | Description |
| --- | --- |
| `--ci` | Convenience bundle: implies `--fail-on high`; guarantees no prompts. |
| `--fail-on low\|medium\|high\|critical` | Exit code 2 when any finding reaches this severity. |
| `--baseline <FILE>` | Report only findings that are new relative to this baseline JSON. |
| `--save-baseline <FILE>` | Write current results as a new baseline. |
| `--annotate github\|gitlab\|sarif` | Emit GitHub Actions workflow commands, GitLab Code Quality JSON, or SARIF in addition to normal output. |

Exit codes: `0` clean · `1` warnings-only · `2` blocking findings · non-zero tool error.

### GitHub Actions example

```yaml
name: sql-analysis
on: [pull_request]
jobs:
  analyze:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: actions/setup-node@v4
        with:
          node-version: 20
      - run: npm install -g sql-optimizer-cli
      - name: Analyze changed SQL
        env:
          SQL_OPTIMIZER_DB_URL: ${{ secrets.SQL_OPTIMIZER_DB_URL }}
        run: |
          sql-optimizer-cli scan ./migrations --ci --annotate github --baseline baseline.json || exit $?
```

### GitLab CI example

```yaml
sql-analysis:
  stage: test
  image: node:20
  script:
    - npm install -g sql-optimizer-cli
    - sql-optimizer-cli batch --input queries.sql --ci --annotate gitlab --output json
```

## Configuration File

`.sql-optimizer.toml` (project root) provides defaults; CLI flags win over it:

```toml
fail_on = "high"          # same values as --fail-on
annotate = "github"       # same values as --annotate
exclude = ["vendor/", "node_modules/", "*.fixture.sql"]
```

## Local State & Tracking

The tool is stateless by default. Regression tracking activates when you pass `--track` **or** a `.sql-optimizer/` directory exists in the project. State lives in `.sql-optimizer/history.sqlite` — add it to `.gitignore`. Schema drift uses `.sql-optimizer/schema-snapshot.json` (created via `schema --save`).

## Troubleshooting

**npm install / the command**

| Symptom | Cause → Fix |
|---|---|
| `sql-optimizer-cli: command not found` after install | npm's global bin dir is not on your PATH → `npm prefix -g` shows it; add `<prefix>/bin` to PATH (node version managers usually handle this). |
| `No prebuilt binary for <platform>` | npm skipped the platform package → check `npm config get omit` (must not include `optional`); corporate registry mirrors sometimes filter platform packages. Fix: `npm install -g sql-optimizer-cli --include=optional` or remove `omit` from `.npmrc`. |
| `EACCES` during global install | Don't use sudo. Point npm's prefix at a user directory (`npm config set prefix ~/.npm-global`) or use a node version manager (nvm/fnm/asdf). |
| 404 on `npm view`/`npm install` | You may be behind a mirror that hasn't synced yet → check against the official registry: `npm view sql-optimizer-cli version --registry=https://registry.npmjs.org`. |
| Wrapper installed but binary is an old version | A pinned/cached platform package → force a clean reinstall: `npm uninstall -g sql-optimizer-cli && npm install -g sql-optimizer-cli@latest`. |

**The TUI**

| Symptom | Cause → Fix |
|---|---|
| `Failed to enable raw mode (is this a terminal?)` | The TUI needs a real TTY — it cannot run through pipes, CI logs, or IDE output panes. Run it in a normal terminal window. |
| TUI starts but shows `Not connected` | Expected when no `--db` was given — use the **Connect** tab: press `Enter` on a provider (SQLite needs no server) or `a` to paste any connection URL. |
| Connection failed inside the TUI (TLS/certificate, timeout, refused) | The TUI stays open on the Connect tab with the URL in the session catalog. For TLS validity errors check your system clock and `sslmode`; for self-signed dev certs restart with `--accept-invalid-certs`. For timeouts check host/port/firewall. |
| Garbled rendering / broken colors | Set `TERM` correctly (`xterm-256color`), or try `sql-optimizer-cli tui --db ...` after resizing the terminal. |

**General**

- Add `-v` to any command for verbose progress and connection details.
- Exit codes: `0` clean · `1` findings (non-blocking) · `2` blocking findings (`--fail-on` exceeded) · `3` tool error (bad connection, missing file, …).
- Connection failures usually mean the URL/host/port/SSL mode is wrong for your network — see the shared flags above (`--db-sslmode`, `--accept-invalid-certs` for self-signed certs in dev, `--simple-mode` behind PgBouncer).

## Requirements

- Node.js 18+ (npm install only; the binary itself has no runtime deps)
- Rust 1.75+ (only when building from source)
- PostgreSQL 12+, MySQL 8.0+, or SQLite
- Network access to target databases
- Linux or macOS (Windows is not supported)

## Development

```bash
cargo build            # build
cargo test             # unit + integration tests
cargo clippy           # lints
cargo fmt              # formatting
make check             # fmt + clippy + test
```

## Releasing (maintainers)

The npm package is built from this repo's Rust binary: `npm/package.json` + `npm/bin/cli.js` (wrapper) plus 4 prebuilt platform packages (`sql-optimizer-cli-{darwin-arm64,darwin-x64,linux-x64,linux-arm64}`), published by `.github/workflows/release-npm.yml` when a `vX.Y.Z` tag is pushed. Platform packages always publish before the wrapper; both steps skip versions already on npm, so tag re-runs are safe.

Release flow:

1. Bump versions everywhere at once: `agent/versioning/scripts/bump-version.sh X.Y.Z` (syncs `Cargo.toml`, the wrapper, and the 4 platform dependency pins).
2. Update the changelog.
3. `make check`, then follow `agent/versioning/checklists/release-checklist.md`.
4. `git tag vX.Y.Z && git push origin vX.Y.Z` — CI builds all targets and publishes.
5. Verify: `npm view sql-optimizer-cli version`, then `npm install -g sql-optimizer-cli` in a clean shell and run `sql-optimizer-cli --version`.

Local npm-package test without publishing: `scripts/release-npm.sh X.Y.Z` assembles all platform packages under `npm/platform/` (add `--publish` to publish).

## License

MIT License — see LICENSE file for details.
