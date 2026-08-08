# SQL Optimization CLI

A command-line tool that analyzes SQL queries and provides optimization recommendations for PostgreSQL, MySQL, and SQLite databases.

## Features

- Parse and analyze SQL queries for performance anti-patterns
- Detect N+1 query patterns, Cartesian products, inefficient JOINs, and missing indexes
- Show execution plans when requested
- Identify security vulnerabilities and injection risks
- Support for PostgreSQL, MySQL, and SQLite databases
- Interactive and batch processing modes
- Query rewriting suggestions
- Text, JSON, YAML, and Markdown output
- Database schema introspection

## Installation

### From Cargo
```bash
cargo install --path . --locked
```

### From Source
```bash
git clone https://github.com/anthonyy616/sql-optimizer-cli.git
cd sql-optimizer-cli
./scripts/install.sh
```

After installation, the main binary is available on your PATH as `sql-optimizer-cli`. The install
script also creates shortcut commands named `analyze`, `batch`, `interactive`, and `schema` in
your Cargo bin directory. During local development, use `cargo run --bin sql-optimizer-cli -- ...`
instead of invoking the binary from `./target/debug`.

Database connections are created per command run. `analyze`, `batch`, and `schema` open a fresh
connection, use it for that command, and then exit. `interactive` keeps the connection open for
the lifetime of that interactive session only.

### Quick Start

The CLI supports two connection styles:

1. Pass a full connection URL with `--db`.
2. Set `SQL_OPTIMIZER_DB_*` values in a `.env` file and run the command without typing the
   password inline.

Example `.env` values:

```bash
SQL_OPTIMIZER_DB_HOST=db.example.supabase.co
SQL_OPTIMIZER_DB_PORT=5432
SQL_OPTIMIZER_DB_USER=postgres
SQL_OPTIMIZER_DB_PASSWORD=your_password_here
SQL_OPTIMIZER_DB_NAME=postgres
SQL_OPTIMIZER_DB_SSLMODE=require
SQL_OPTIMIZER_DB_ACCEPT_INVALID_CERTS=false
```

For Supabase, prefer the session pooler connection string for short-lived CLI runs. If your
environment is IPv4-only or your database hostname is not reachable over IPv6, use the IPv4
endpoint instead of a hostname that resolves only on IPv6. When connecting through a pooler,
add `--simple-mode` so the client avoids prepared statements.

The fastest smoke test is the schema command:

```bash
cargo run -- schema --db "$SQL_OPTIMIZER_DB_URL"
```

If your Supabase or pooler certificate chain is not trusted in WSL, you can explicitly opt into
trusting it for local testing:

```bash
cargo run -- schema --accept-invalid-certs
```

**Analyze a Single Query**
```bash
sql-optimizer-cli analyze "SELECT * FROM users WHERE email = 'test@example.com'" --db postgresql://user:password@localhost:5432/mydb
```

**Interactive Mode**
```bash
sql-optimizer-cli interactive --db postgresql://user:password@localhost:5432/mydb
```

**Batch Analysis**
```bash
sql-optimizer-cli batch --input queries.sql --output-file recommendations.json --db mysql://user:password@localhost:3306/mydb
```

## Usage

**Connection Strings**
The tool supports standard connection strings for PostgreSQL and MySQL:
```bash
PostgreSQL: postgresql://[user[:password]@][host][:port][/dbname][?param1=value1&...] MySQL: mysql://[user[:password]@][host][:port][/dbname][?param1=value1&...]
```

For Supabase and similar hosted Postgres services, the session pooler URL is usually the best
fit for this CLI's short-lived connections. If your setup needs IPv4, use the IPv4 endpoint or
direct host that your network can actually reach.

When using `.env`, place the file in the directory you run the CLI from. The CLI reads
environment values automatically at startup.

**TLS / Supabase troubleshooting**

If WSL rejects the certificate chain for your Supabase host, use `--accept-invalid-certs` only for
local testing. Keep it off for normal use.

### **Command Reference**

**analyze**
Analyze a single SQL query:

```bash
sql-optimizer-cli analyze "SELECT u.*, o.total FROM users u JOIN orders o ON u.id = o.user_id" --db postgresql://localhost/mydb
```

**Options:**

**--explain**: Show execution plan

**--output json|yaml|text**: Output format

**--verbose**: Detailed analysis information

**--accept-invalid-certs**: Allow a self-signed or otherwise untrusted TLS certificate chain for
PostgreSQL connections

**interactive**
Start an interactive session:
```bash
sql-optimizer-cli interactive --db postgresql://localhost/mydb --history ~/.sql-history
```

**batch**
Process multiple queries from a file:

```bash
sql-optimizer-cli batch --input queries.sql --output results.json --db mysql://localhost/mydb
```

### Reusable Query Scripts

Save these as shell scripts if you want a repeatable way to run the CLI with your env vars:

```bash
#!/usr/bin/env bash
set -euo pipefail

sql-optimizer-cli analyze "$1" \
  --db "${SQL_OPTIMIZER_DB_URL}" \
  --simple-mode
```

  For Supabase, prefer the session pooler connection string for short-lived CLI runs. If your
  environment is IPv4-only or your database hostname is not reachable over IPv6, use the IPv4
  endpoint instead of a hostname that resolves only on IPv6. When connecting through a pooler,
  add `--simple-mode` so the client avoids prepared statements.
sql-optimizer-cli batch \
  --input queries.sql \
  --output recommendations.json \
  --db "${SQL_OPTIMIZER_DB_URL}" \
  cargo run --bin sql-optimizer-cli -- schema --db "$SQL_OPTIMIZER_DB_URL"
```

```bash
#!/usr/bin/env bash
set -euo pipefail

  cargo run --bin sql-optimizer-cli -- schema --accept-invalid-certs
  --db "${SQL_OPTIMIZER_DB_URL}" \
  --simple-mode
  ## Command Reference

  ### Shared Flags

  These flags are accepted by every subcommand because they come from the shared `ConnectionArgs`
  structure or the top-level CLI.

  | Flag | Description |
  | --- | --- |
  | `-v`, `--verbose` | Print extra progress details before running the command. |
  | `-d`, `--db <URL>` | Full database connection string. If this is set, the individual connection parts are ignored. |
  | `--db-host <HOST>` | Hostname used when building a connection string from parts. |
  | `--db-port <PORT>` | Port used when building a connection string from parts. |
  | `--db-user <USER>` | Username used when building a connection string from parts. |
  | `--db-password <PASSWORD>` | Password used when building a connection string from parts. |
  | `--db-name <NAME>` | Database name used when building a connection string from parts. |
  | `--db-sslmode <MODE>` | PostgreSQL SSL mode used when building a connection string from parts. Defaults to `require`. |
  | `--accept-invalid-certs` | Allow self-signed or otherwise invalid certificates when connecting to PostgreSQL. |

  ### `analyze`

  Analyze a single SQL query.

  Syntax:

  ```bash
  sql-optimizer-cli analyze <QUERY> [shared flags] [--explain] [--output <FORMAT>] [--simple-mode] [--connect-timeout <SECONDS>]
  ```

  | Flag | Description |
  | --- | --- |
  | `QUERY` | Required SQL statement to analyze. |
  | `--explain` | Include an execution plan in the output. |
  | `--show-rows` | Preview matching rows for a read-only `SELECT`. |
  | `--row-limit <N>` | Limit the number of preview rows shown when `--show-rows` is enabled. |
  | `-o`, `--output <FORMAT>` | Output format. Valid values are `text`, `json`, `yaml`, and `markdown`. Defaults to `text`. |
  | `--simple-mode` | Force simple queries and avoid prepared statements. Useful for PgBouncer transaction pooling. |
  | `--connect-timeout <SECONDS>` | Connection timeout in seconds. |

  Example:

  ```bash
  sql-optimizer-cli analyze "SELECT u.*, o.total FROM users u JOIN orders o ON u.id = o.user_id" \
    --db postgresql://user:password@localhost:5432/mydb \
    --explain \
    --output markdown \
    --simple-mode
  ```

  ### `interactive`

  Start an interactive session that keeps the connection open while you enter queries.

  Syntax:

  ```bash
  sql-optimizer-cli interactive [shared flags] [--history <PATH>] [--output <FORMAT>] [--simple-mode] [--connect-timeout <SECONDS>]
  ```

  | Flag | Description |
  | --- | --- |
  | `--history <PATH>` | History file path. Defaults to `~/.sql-optimizer-history`. |
  | `--show-rows` | Preview matching rows for each analyzed query. |
  | `--row-limit <N>` | Limit the number of preview rows shown when `--show-rows` is enabled. |
  | `-o`, `--output <FORMAT>` | Output format. Valid values are `text`, `json`, `yaml`, and `markdown`. Defaults to `text`. |
  | `--simple-mode` | Force simple queries and avoid prepared statements. |
  | `--connect-timeout <SECONDS>` | Connection timeout in seconds. |

  Example:

  ```bash
  sql-optimizer-cli interactive --db postgresql://user:password@localhost:5432/mydb --history ~/.sql-optimizer-history
  ```

  ### `batch`

  Process multiple queries from a file and write the results to a file.

  Syntax:

  ```bash
  sql-optimizer-cli batch --input <FILE> [shared flags] [--output-file <FILE>] [--output <FORMAT>] [--simple-mode] [--connect-timeout <SECONDS>]
  ```

  | Flag | Description |
  | --- | --- |
  | `-i`, `--input <FILE>` | Input file containing SQL queries. |
  | `--output-file <FILE>` | Explicit JSON file for the recommendations. |
  | `-o`, `--output <FORMAT>` | Result format. Valid values are `text`, `json`, `yaml`, and `markdown`. Defaults to `text`. |
  | `--simple-mode` | Force simple queries and avoid prepared statements. |
  | `--connect-timeout <SECONDS>` | Connection timeout in seconds. |

  Example:

  ```bash
  sql-optimizer-cli batch --input queries.sql --output-file recommendations.json --db mysql://user:password@localhost:3306/mydb
  ```

  ### `schema`

  Connect to a database and print the introspected schema.

  Syntax:

  ```bash
  sql-optimizer-cli schema [shared flags] [--simple-mode] [--connect-timeout <SECONDS>]
  ```

  | Flag | Description |
  | --- | --- |
  | `--simple-mode` | Force simple queries and avoid prepared statements. |
  | `--connect-timeout <SECONDS>` | Connection timeout in seconds. |

  Example:

  ```bash
  sql-optimizer-cli schema --db sqlite::memory:
  ```

  ## Notes

  `--show-rows` and `--row-limit` control row previews for `analyze` and `interactive`. Batch uses
  `--output-file` for an explicit JSON destination, and `--output` selects the rendered format for
  all three commands. When `batch` is run with a non-text `--output` and no explicit output file,
  the CLI writes an auto-named file under `output/`.

  ## Connection Strings
#!/usr/bin/env bash
  The tool supports standard connection strings for PostgreSQL, MySQL, and SQLite:
  "database": "postgresql",
  ```bash
  PostgreSQL: postgresql://[user[:password]@][host][:port][/dbname][?param1=value1&...]
  MySQL: mysql://[user[:password]@][host][:port][/dbname][?param1=value1&...]
  SQLite: sqlite::memory: or sqlite:///path/to/local.db
  ```

  For Supabase and similar hosted Postgres services, the session pooler URL is usually the best
  fit for this CLI's short-lived connections. If your setup needs IPv4, use the IPv4 endpoint or
  direct host that your network can actually reach.

  When using `.env`, place the file in the directory you run the CLI from. The CLI reads
  environment values automatically at startup.

  ## Reusable Query Scripts

  Save these as shell scripts if you want a repeatable way to run the CLI with your env vars:
    {
      "type": "missing_index",
      "table": "users",
      "columns": ["email"],
      "estimated_improvement": 0.73
  sql-optimizer-cli analyze "$1" \
  ],
  "security": {
    "score": 100,
    "issues": []
  }
}
```

## Development

### Building
```bash
cargo build
```

## Running Tests
```bash
cargo test
```

## Requirements
Rust 1.70 or higher
PostgreSQL 12+ or MySQL 8.0+
Network access to target databases

## License
MIT License - see LICENSE file for details