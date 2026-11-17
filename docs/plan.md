# LazyDB — Plan

Phases from [vision.md](vision.md) §6. Each step ends with `mise run ci`
green and a commit.

**Last updated:** 2026-11-16

---

## Phase 1 — Merge (done)

LazyPg 0.2.1 renamed (`lp-*` → `ld-*`), then:

1. **Shared model.** Keys carry a role per column, columns a kind, schemas
   their kind from the engine. Sequences, functions and partitions dropped.
2. **Engine-aware config.** `engine` per connection, inferred when unstated;
   LazyCassandra's keys, TLS model and `cqlshrc` ported.
3. **Engine-aware domain.** Statements carry their engine; the editor's
   lexer, keywords, templates, resolver, splitter and formatter take it.
   Literals, cell edits, row keys and DDL per engine; collections in `Value`.
4. **Cassandra adapter.** `system_schema` read as data, scylla paging, the
   `Engines` router, `Cluster` port renamed `Database`.
5. **Structure for Cassandra.** Partition and clustering sections, keyspace
   labels, static columns; the grid empties on reconnect.
6. **Structure tabs.** Keys folded into Indexes; an Options tab for tables
   that have options, with compaction and compression read as class and
   settings; keyspace replication one row per factor; clustering and static
   colours back; DDL highlighted.

## Phase 2 — More SQL engines

Per engine, in this order: MySQL/MariaDB, SQL Server, Oracle.

1. `Engine` variant; the compiler lists every seam (architecture.md §6).
2. Config target and validation; a URL form where the ecosystem has one.
3. Catalog queries in `introspection/`, mapped onto the shared model.
4. Dialect: identifier quoting (backticks, brackets), comments, string
   escapes, keywords, templates, multi-row atomicity, classification.
5. Adapter and fixture; an integration test that round-trips DDL and yanked
   rows, as the Cassandra one does.

Driver choice is open per engine: it must be pure Rust (conventions.md §1).

## Open

- **Staged edits on reconnect.** Connecting elsewhere drops them with the
  grid; a discard prompt, as quitting has, would be kinder.
- **Postgres fixture flakes.** Under a full parallel run,
  `every_value_read_back_spells_a_literal…` with
  `the_probe_follows_a_transaction…` (both write `zoo.everything`), and
  `the_catalog_places_itself_on_a_server…`, each failed once and passed on
  every rerun. Cause not found.
- **Cassandra cancellation.** `Effect::Cancel` aborts the task but the
  driver sends no cancel; a superseded read runs to completion server-side.
- **Exact paging for Postgres** (keyset, portals) and **several connections
  open at once**, carried over from LazyPg.
