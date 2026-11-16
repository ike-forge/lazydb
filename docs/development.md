# Developing LazyDB

Tool versions are pinned in [mise.toml](../mise.toml); `mise install`, then
the tasks listed in [AGENTS.md](../AGENTS.md).

## Fixtures

`mise run db:up` starts both servers under the `lazydb` compose project and
seeds them on first start:

- **Postgres 18** on `localhost:5433` (`LD_PG_PORT` moves it), from
  `docker/sql`: `shop` (keys, composite key, foreign keys, indexes, a view),
  `zoo` (one column per type family), `"Edge Cases"` (quoted names, a keyless
  table, a materialized view, a partitioned table), and a second database,
  `analytics`, for switching.
- **Cassandra 5.0** on `localhost:9043` (`LD_CASSANDRA_PORT`), from
  `docker/cql` via `docker/seed-cql.sh`: `shop` (clustering order, indexes, a
  materialized view, counters), `typezoo` (every type, static columns,
  collections), `edge_cases`.

`mise run db:psql` and `mise run db:cqlsh` open shells; `mise run db:down`
drops the volumes, so the next `db:up` re-seeds.
[.lazydb.toml](../.lazydb.toml) points the app at both (`local` and
`cassandra`) with writes enabled.

`mise run test:it` runs the integration tier against them.

## Releasing

Run the **Prepare release** workflow on `main`. It requires a green CI run
on that commit, bumps `[workspace.package] version`, tags `v<version>` and
dispatches `release.yml`, which builds with cargo-dist and pushes `lazydb.rb`
to `ike-forge/homebrew-tap` using the `HOMEBREW_TAP_TOKEN` secret.

`release.yml` is generated: change `dist-workspace.toml`, then run
`dist generate`.
