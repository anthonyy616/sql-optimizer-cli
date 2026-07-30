# Cloud Postgres (Supabase / Neon)

This page explains how to run `sql-optimizer-cli` against cloud Postgres providers such as Supabase and Neon, and what runtime flags help when a connection pooler (pgbouncer) or serverless cold-starts are present.

Key flags:

- `--simple-mode`: avoids server-side prepared statements by using simple queries. Use this when the target database is behind a pgbouncer transaction-mode pooler that rejects prepared statements.
- `--connect-timeout <seconds>`: increase the connection timeout to allow for Neon cold-start latency (default 25s).
- `--verbose`: show more connection/logging details.

Examples

Analyze a single query against a Supabase URL (uses TLS automatically when `sslmode=require` in the URL):

```bash
./target/debug/sql-optimizer-cli analyze "SELECT id FROM users WHERE email = 'x@example.com'" --db "postgresql://USER:PASS@HOST:PORT/DATABASE?sslmode=require" --simple-mode --connect-timeout 60 --verbose
```

If your Supabase/Neon project requires `sslmode=require`, include that parameter in the connection URL. If you see connection timeouts, increase `--connect-timeout`.

Troubleshooting

- If you see errors about prepared statements or `prepared statement "..." does not exist`, try adding `--simple-mode`.
- If connections appear to hang initially, this may be Neon cold-start latency. Increase `--connect-timeout` and run again; use `--verbose` to see detailed connection timing.

Automated test script

See `scripts/test_cloud_postgres.sh` for a small helper script that runs `analyze` against a URL provided via environment variable `CLOUD_DB_URL`.
