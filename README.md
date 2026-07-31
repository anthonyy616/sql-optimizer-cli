# SQL Optimization CLI

A command-line tool that analyzes SQL queries and provides optimization recommendations for PostgreSQL and MySQL databases.

## Features

- Parse and analyze SQL queries for performance anti-patterns
- Detect N+1 query patterns, Cartesian products, and inefficient JOINs
- Suggest index creation opportunities
- Identify security vulnerabilities and injection risks
- Support for PostgreSQL and MySQL databases
- Interactive and batch processing modes
- Query rewriting suggestions

## Installation

### From Cargo
```bash
cargo install --path . --locked
```
From Source
```bash
git clone https://github.com/anthonyy616/sql-optimizer-cli.git
cd sql-optimizer-cli
./scripts/install.sh
```

After installation, the main binary is available on your PATH as `sql-optimizer-cli`, and the
install step also creates shortcut commands named `analyze`, `batch`, `interactive`, and
`schema` in your Cargo bin directory. During local development, use `cargo run -- analyze ...`
instead of invoking the binary from `./target/debug`.

Database connections are created per command run. `analyze`, `batch`, and `schema` open a fresh
connection, use it for that command, and then exit. `interactive` keeps the connection open for
the lifetime of that interactive session only.

### Quick Start

The CLI now supports two connection styles:

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
sql-optimizer-cli batch --input queries.sql --output recommendations.json --db mysql://user:password@localhost:3306/mydb
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

```bash
#!/usr/bin/env bash
set -euo pipefail

sql-optimizer-cli batch \
  --input queries.sql \
  --output recommendations.json \
  --db "${SQL_OPTIMIZER_DB_URL}" \
  --simple-mode
```

```bash
#!/usr/bin/env bash
set -euo pipefail

sql-optimizer-cli interactive \
  --db "${SQL_OPTIMIZER_DB_URL}" \
  --simple-mode
```

```bash
#!/usr/bin/env bash
set -euo pipefail

sql-optimizer-cli schema \
  --db "${SQL_OPTIMIZER_DB_URL}" \
  --simple-mode
```

## Output Examples
### Text Output

```bash
SQL Analysis Results
===================
Query: SELECT * FROM users WHERE email = 'test@example.com'
Database: PostgreSQL 14.2
Analysis Time: 0.8s

OPTIMIZATION OPPORTUNITIES:
- Missing Index: CREATE INDEX idx_users_email ON users(email)
  Estimated improvement: 73% faster

SECURITY ANALYSIS:
- No security issues detected
```

### JSON Output
```json
{
  "query": "SELECT * FROM users WHERE email = 'test@example.com'",
  "database": "postgresql",
  "recommendations": [
    {
      "type": "missing_index",
      "table": "users",
      "columns": ["email"],
      "estimated_improvement": 0.73
    }
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