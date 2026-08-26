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
- **TUI dashboard** — full-screen interactive mode (`tui`) for analyze/schema/health/history

## Installation

### From Source
```bash
git clone https://github.com/anthonyy616/sql-optimizer-cli.git
cd sql-optimizer-cli
./scripts/install.sh
```

The install script puts `sql-optimizer-cli` on your PATH and creates shortcut commands named `analyze`, `batch`, `interactive`, `schema`, `scan`, and `tui`. During local development use `cargo run --bin sql-optimizer-cli -- ...`.

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
sql-optimizer-cli schema --db "$SQL_OPTIMIZER_DB_URL"

# Analyze a query with plan + fix suggestions
sql-optimizer-cli analyze \
  "SELECT u.*, o.total FROM users u JOIN orders o ON u.id = o.user_id" \
  --db postgresql://user:pass@localhost:5432/mydb --explain

# Health snapshot (top queries by time, table sizes)
sql-optimizer-cli health --db "$SQL_OPTIMIZER_DB_URL"

# Scan a whole project: .sql files, dbt models, app source, slow logs
sql-optimizer-cli scan ./migrations --db "$SQL_OPTIMIZER_DB_URL" --output json

# Full-screen dashboard
sql-optimizer-cli tui --db "$SQL_OPTIMIZER_DB_URL"
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
sql-optimizer-cli analyze <QUERY> [shared flags] [--explain] [--show-rows] [--row-limit N] \
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
sql-optimizer-cli batch --input queries.sql [--output-file FILE] [-o FORMAT] [shared flags] [CI flags]
```

With a non-text `--output` and no explicit output file, results are auto-written under `output/`.

### `scan`

Scan a file or directory: raw `.sql` files, migration files, dbt models (Jinja `{{ ref() }}` stripped best-effort), application source containing embedded SQL, and Postgres/MySQL log formats. Queries are fingerprinted and deduplicated; the report surfaces top offenders with origin file/line. Never prompts — safe for CI.

```bash
sql-optimizer-cli scan <PATH> [shared flags] [-o FORMAT] [--output-file FILE] [--schema-baseline FILE] [CI flags]
```

Exclusions come from `exclude` in `.sql-optimizer.toml`.

### `schema`

Introspect and print the schema as a tree (tables → columns → indexes → FKs).

```bash
sql-optimizer-cli schema [shared flags] [--save <FILE>]
```

`--save <FILE>` writes the snapshot JSON, which can later be passed to `--schema-baseline` for drift detection.

### `health`

Point-in-time DB health snapshot: top queries by total time (`pg_stat_statements` / `performance_schema`) and table cardinality. If the extension or privilege isn't available, the command still succeeds and says so explicitly — it is not a monitoring daemon.

```bash
sql-optimizer-cli health [shared flags]
```

### `interactive`

Classic line-based interactive session; keeps one connection open for the session.

```bash
sql-optimizer-cli interactive [shared flags] [--history ~/.sql-optimizer-history] [--show-rows] [-o FORMAT]
```

### `tui`

Full-screen terminal dashboard with four panels:

- **Analyze** — type a query, press Enter; results include recommendations, security findings, and plan summary
- **Schema** — press `s` to refresh the introspected tree
- **Health** — press `h` for a live stats snapshot
- **History** — press `r` to list recent tracked runs from the local state store

Keys: `Tab`/`←→` switch panels, `↑↓` scroll, `q`/`Esc` quit. Requires a working database connection and a real terminal.

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
      - run: cargo install --path .
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
  image: rust:latest
  script:
    - cargo install --path .
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

## Requirements

- Rust 1.75+
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

## License

MIT License — see LICENSE file for details.
