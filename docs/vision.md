# LazyDB — Vision

> One terminal UI for the databases you work with, in the tradition of
> LazyGit and k9s.

**Last updated:** 2026-11-16

---

## 1. Why

LazyPg and LazyCassandra were the same tool twice: the same panels, keys,
editor, grid, inspector, export and write flow, over a different driver and
language. Keeping two binaries meant two configs, two muscle memories for
the same keys, and every fix made twice.

Most of a working day does not need what makes an engine special. It needs
the structure of a table beside its rows, a query with completion, an edit
behind a preview, and an export. LazyDB keeps those and leaves the rest to
each engine's own tooling.

## 2. Scope: what engines share

A feature belongs here when every supported engine has it, under whatever
name:

| Concept | Postgres | Cassandra |
|---|---|---|
| Namespace | schema (inside a database) | keyspace |
| Relation | table, view, materialized view | table, materialized view |
| Row identity | primary key | partition key + clustering key |
| Column | type, nullability, default | type, static |
| Index | b-tree, GIN, … with predicate | secondary, SAI, SASI |
| Statement | SQL | CQL |
| Atomic multi-row write | transaction | logged batch |
| Paging | `LIMIT/OFFSET` | driver paging state |

Concepts most SQL engines share but Cassandra lacks — foreign keys, check
and unique constraints, transactions — are kept where they exist and simply
absent elsewhere. They will matter again for MySQL, SQL Server and Oracle.

**Out of scope**, because they belong to one engine: query plans
(`EXPLAIN`), server activity and locks, vacuum and bloat; table health,
tombstones, compaction, tracing and per-request consistency switching;
sequences, functions, partitions as objects, replication and table options.

## 3. Who it's for

- **The application developer** who needs to see what a table looks like,
  how it is keyed and what references it before writing against it.
- **The on-call engineer** who needs to look at rows, quickly and safely.
- **Anyone switching engines in a day**, who wants the same keys everywhere.

## 4. Interface principles

1. **Discoverability is ambient, not modal.** The bottom bar shows what can
   be done to the focused thing. `?` shows the contextual map.
2. **The same key at every level of skill.** No hidden fast path.
3. **Show the statement. Always.** Every action is shown as SQL or CQL before
   it runs and logged after.
4. **Panels are places.** Persistent, numbered panels in fixed positions.
5. **Colour is a relationship.** Themes declare their own ground and are held
   to 4.5:1 contrast by a test.
6. **Destructive things are obvious, reversible, or confirmed — pick two.**
   Read-only by default; drops need typed confirmation.
7. **Speed is a feature of the interface.** The UI thread never touches the
   server.
8. **One vocabulary, the engine's words.** Panels speak the shared model;
   labels, statements and the editor use the engine's own terms.

## 5. Layout

| # | Panel | Holds |
|---|---|---|
| 1 | Objects | databases → schemas, or keyspaces → tables, views |
| 2 | Structure | columns, keys, indexes, DDL |
| 3 | Contents | the result grid: table rows or query results, paged |
| 4 | Queries | editor and session log |

## 6. Roadmap

- **Phase 1 — Merge.** *Done.* LazyPg's shell over a shared model, with
  Postgres and Cassandra adapters behind one router.
- **Phase 2 — More SQL engines.** MySQL/MariaDB, then SQL Server and Oracle.
  Each is an `Engine` variant, a dialect (lexer rules, keywords, literals,
  DDL) and an adapter; architecture.md §6 lists the seams.
- **Phase 3 — Extend.** User keybindings, saved queries, several connections
  open at once.

## 7. Non-goals

- Not a GUI or web app.
- Not a lowest-common-denominator SQL client: each engine's statements are
  its own language, highlighted and completed as such.
- Not a migration, ETL, backup or monitoring tool.
