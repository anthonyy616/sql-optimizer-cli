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
cargo install sql-optimizer-cli
```
From Source
```bash
git clone https://github.com/anthonyy616/sql-optimizer-cli.git
cd sql-optimizer-cli
cargo build --release
cp target/release/sql-optimizer-cli ~/.local/bin/
```

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

The fastest smoke test is the schema command:

```bash
cargo run -- schema
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