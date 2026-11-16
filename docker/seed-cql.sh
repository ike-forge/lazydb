#!/usr/bin/env bash
#
# Seeds the Cassandra fixture keyspaces. Runs inside the `cassandra-seed`
# service, which waits for the node to report healthy.
#
# Idempotent: all DDL is IF NOT EXISTS and every INSERT an upsert with a
# literal primary key. Counters are the exception, guarded by a read.

set -euo pipefail

HOST="${CQLSH_HOST:-cassandra}"
PORT="${CQLSH_PORT:-9042}"
CQL_DIR="${CQL_DIR:-/fixture/cql}"

cql() {
    cqlsh "$HOST" "$PORT" --request-timeout=120 "$@"
}

for f in "$CQL_DIR"/*.cql; do
    printf '  %s\n' "$(basename "$f")"
    cql -f "$f"
done

# `SET x = x + n` is an increment, so replaying it would multiply the totals.
if cql -e "SELECT product_id FROM shop.product_view_counts LIMIT 1;" | grep -q '(0 rows)'; then
    cql -e "
UPDATE shop.product_view_counts SET views = views + 18452, add_to_cart = add_to_cart + 903
    WHERE product_id = b0000000-0000-4000-8000-000000000001 AND day = '2026-03-01';
UPDATE shop.product_view_counts SET views = views + 3, add_to_cart = add_to_cart + 0
    WHERE product_id = b0000000-0000-4000-8000-000000000006 AND day = '2026-03-01';"
fi
