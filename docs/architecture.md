# LazyDB — Architecture

Companion to [vision.md](vision.md). The structure is LazyPg's, itself
LazyCassandra's; this document records what is shared, where an engine
differs, and how one is added.

**Last updated:** 2026-11-16

---

## 1. Decisions

| Decision | Choice | Why |
|---|---|---|
| Language | Rust | Latency floor, single static binary, type-safe long-lived state |
| TUI | `ratatui` + `crossterm` | Unopinionated about state, good test backend |
| State model | Elm-style (TEA) | Pure `update`, headless tests, async results as messages |
| Structure | Hexagonal — domain / inbound / outbound / app | Compiler-enforced boundaries |
| Engines | One `Engine` enum, matched exhaustively | A new engine fails to compile until every seam answers for it (§6) |
| Drivers | `tokio-postgres`, `scylla` | Pure Rust; LazyPg's and LazyCassandra's choices, kept |
| Base | LazyPg 0.2.1 | It was ahead of LazyCassandra in the shared shell |

## 2. Shape

```
crates/
  ld-domain/    # pure core. std, serde, thiserror, async-trait, unicode-*
  ld-inbound/   # TUI adapter. ld-domain, ratatui, crossterm
  ld-outbound/  # drivers, config, secrets, history store. ld-domain, tokio
  ld-app/       # binary `lazydb`: composition root, effect runtime, main loop
```

`ld-domain` lists no terminal, runtime or driver. `ld-inbound` and
`ld-outbound` never see each other.

## 3. The loop

`update(&mut State, Msg) -> Vec<Effect>`, effects interpreted by `ld-app`,
every async result re-entering as a `Msg`, frame geometry the one backwards
edge, messages batched per frame. Unchanged from LazyCassandra
architecture.md §3.

## 4. The shared model

`ld_domain::model` is the vocabulary every panel speaks; adapters map onto
it and stop there.

| Type | Postgres | Cassandra |
|---|---|---|
| `Catalog { engine, server, schemas }` | `server` lists the databases | `server` is `None` |
| `Schema { name, kind, tables }` | schema | keyspace |
| `SchemaKind::of(engine, name)` | `pg_*`, `information_schema` | `system`, `system_*` |
| `RelationKind` | table (incl. partitioned, foreign), view, materialized view | table, materialized view |
| `PrimaryKey { name, columns: [KeyColumn { role }] }` | every role `Plain`, named | `Partition`, then `Clustering(order)`, unnamed |
| `Column { default, kind }` | `ColumnDefault`, always `Regular` | no default, `Static` or `Regular` |
| `Index { keys, method, predicate, definition }` | `pg_get_indexdef` | rebuilt `CREATE [CUSTOM] INDEX`, class as `method` |
| `ForeignKey`, `Constraint` | read | none |
| `Options` on `Table` and `Schema` | none | table options; keyspace durability and replication, spread one factor a row |
| `Value` | scalars, `Numeric`, `Array`, `Json` as text | scalars, `Numeric`, `List`, `Set`, `Map`; blobs as `0x…` text |
| `Statement { sql, kind, engine }` | SQL | CQL |
| `PagingState` | offset | driver bytes |
| `Transaction` | probed after writes | always `Idle` |

Dropped in the merge, as one engine's: sequences, functions, partitions;
table health, traces, the consistency picker.

The Structure panel's tabs are Columns, Indexes (the primary key — split into
partition and clustering for Cassandra — references, constraints, then the
other indexes: a key is an index too), Options where the table has any, and
DDL, highlighted as the editor highlights it. Clustering columns and static
columns have their own colours (`clustering` role, `accent`).

**Posture gates effects in the domain.** A statement classified as mutating
on a read-only connection never becomes an `Effect`. Postgres adds a server
rail: read-only sessions set `default_transaction_read_only = on`.
Classification differs per engine (`model/statement.rs`): Postgres reads
every word of every statement, so a data-modifying `WITH` or
`EXPLAIN ANALYZE UPDATE` is a write; CQL reads each statement's verb.

## 5. Ports

| Port | Responsibility |
|---|---|
| `Database` | connect, describe, transaction state, execute, execute paged |
| `ConfigSource` / `ConfigSink` | layered config; theme write-back |
| `SecretResolver` | literal / env / file; `~/.pgpass` and `cqlshrc` in their adapters |
| `Store` | query history and saved queries: statement, time, engine, database per entry, a name per saved one |
| `Writer` | export sinks |

`ld_outbound::engines::Engines` implements `Database` over both adapters and
routes each `ConnId` to the adapter its entry's engine names. The app holds
one `Arc<dyn Database>`.

## 6. Where an engine differs

Every seam is a `match` on `Engine`, so a new variant is a compile error
until each is answered:

| Seam | Module |
|---|---|
| Config keys and validation | `ld-domain/config.rs` (`Target`), `ld-outbound/config/{wire,validate}.rs` |
| Catalog queries, as data | `ld-domain/introspection/` |
| Lexing (quotes, comments, literals, parameters) | `ld-domain/sql/lex.rs` |
| Keywords, types, templates | `ld-domain/sql/{keywords,templates}/` |
| Resolver, splitter, formatter quirks | `ld-domain/sql/{resolve,statements,format}.rs` |
| Classification and built statements | `ld-domain/model/statement.rs` |
| Literals, cell parsing, row keys | `ld-domain/{literal,edit,key}.rs` |
| DDL and identifier quoting | `ld-domain/create/` |
| Labels and capabilities | `ld-domain/engine.rs` |
| Driver adapter | `ld-outbound/{postgres,cassandra}.rs`, routed in `engines.rs` |

The Queries panel holds its buffer's engine, set when a connection is
chosen; everything else asks `State::engine()`. Each history entry records
its engine, so the picker's engine layer spans connections; an entry from a
file older than that takes its connection's engine from the config. Entries
fold as SQL for both engines, which fold names and keywords alike, so one
statement is one row across engines.

`ctrl-r` cycles five layers: default (the connection's saved queries, then
its history, a saved statement listed once), saved (every connection on the
engine), then the three history layers. A query hits a saved name before any
statement: the two scores do not compare. Saved queries sit in `saved.toml`
beside `history.toml`, keyed by connection, one name per connection, case
aside; the history limit does not bound them.

## 7. Paging

Postgres has no resumable paging state: table browse is `LIMIT/OFFSET`
ordered by the primary key, and a read-only statement is wrapped as a
subquery; keyless tables page in heap order and say so. Cassandra pages by
the driver's state, and a table scan carries no `ORDER BY`, which Cassandra
refuses across partitions.

## 8. Configuration

Layered as LazyCassandra architecture.md §7. A connection is
`{ name, posture, target: Target }`; `Target::Postgres` holds libpq's keys,
resolved at connect time as libpq resolves them, `Target::Cassandra` holds
hosts, discovery, datacenter, credentials, TLS and consistency. Unstated,
`engine` is inferred from the keys, so both predecessors' files load.

Postgres scopes a session to one database; opening another reconnects the
same entry with `database` replaced. Connecting anywhere empties the grid,
so its rows cannot be written to the new server.

## 9. Testing

Four tiers: domain unit, scenario (fake `Database`), render (`TestBackend`),
integration (`docker/compose.yaml`: Postgres 18 on 5433, Cassandra 5.0 on
9043, `--features integration`). The Cassandra tier rebuilds every seeded
table's DDL into a scratch keyspace and yanks rows back as a batch, so the
CQL the domain writes is checked by the server.

## 10. Conventions

`thiserror` in the domain, `anyhow` only in `ld-app`, no `unwrap` outside
tests and the composition root, public `ld-domain` items documented, zero
warnings. See [conventions.md](conventions.md).
