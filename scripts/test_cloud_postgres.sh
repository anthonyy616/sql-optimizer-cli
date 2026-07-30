#!/usr/bin/env bash
# Helper to run sql-optimizer-cli analyze against a cloud Postgres (Supabase/Neon)
# Usage: CLOUD_DB_URL="postgresql://user:pass@host:port/db?sslmode=require" ./scripts/test_cloud_postgres.sh

set -euo pipefail

if [ -z "${CLOUD_DB_URL-}" ]; then
  echo "Set CLOUD_DB_URL to the target Postgres URL (include sslmode=require if needed)"
  exit 1
fi

QUERY="SELECT 1"

./target/debug/sql-optimizer-cli analyze "$QUERY" --db "$CLOUD_DB_URL" --simple-mode --connect-timeout 60 --verbose
