# LazyDB

**Your databases, a few keystrokes away.**

LazyDB is one terminal UI for PostgreSQL and Apache Cassandra. Browse
schemas (keyspaces) and tables, see columns, keys, indexes and DDL next to
the rows, run SQL or CQL, and edit data behind a preview of the statements it
will send. Vim-style keys and a lazygit-style panel layout.

It keeps what the two engines share and leaves out what only one has: query
plans, cluster health, tombstones, tracing and consistency switching stay in
the engines' own tools.

- **See the structure.** Postgres databases and schemas, Cassandra keyspaces;
  tables, views and materialized views; columns, primary key (partition and
  clustering for Cassandra), foreign keys, indexes, highlighted DDL, and
  Cassandra's table options and replication.
- **Write SQL or CQL with context.** The editor speaks the connection's
  language: schema-aware completion, highlighting, formatting, `$EDITOR`,
  per-connection history and a session log.
- **Explore and export.** Paging, local sort and filter, full-value
  inspection, clipboard yanks, CSV/JSON export, rows copied as `INSERT`s.
- **Control writes.** Read-only by default. Cell edits, inserts and deletes
  are previewed before they run, as one transaction in Postgres and one
  logged batch in Cassandra.

## Install

PostgreSQL 14 and later; Cassandra 4.x and 5.x. Linux and macOS; on Windows,
use WSL. A true-colour terminal works best; icons need a Nerd Font, or set
`icons = false`.

### From source

Needs Git, a C compiler (for `aws-lc-rs`) and Rust 1.97.1:

```sh
git clone https://github.com/ike-forge/lazydb.git
cd lazydb
rustup toolchain install 1.97.1
cargo +1.97.1 install --path crates/ld-app --locked
```

With [mise](https://mise.jdx.dev/), `mise install` provides the toolchain,
then `cargo install --path crates/ld-app --locked`.

## Get started

The first launch writes a commented `~/.config/lazydb/config.toml`. Add a
connection, then:

```sh
lazydb --connection dev
```

Without `--connection` or `default_connection`, LazyDB opens the connection
picker; `Ctrl-o` reopens it to switch. `Enter` unfolds a schema or opens a
table, `4` opens the editor, `Ctrl-r` runs the statement under the cursor,
`?` lists every command.

## Configuration

Layers, later wins: built-in defaults → `~/.config/lazydb/config.toml`
(`$XDG_CONFIG_HOME/lazydb/config.toml` if set) → the nearest `.lazydb.toml`
in the working directory or an ancestor → `--config <path>`.
`--connection <name>` overrides `default_connection`.

```toml
theme = "catppuccin-mocha"
default_connection = "dev"

[[connections]]
name     = "dev"
engine   = "postgres"             # implied when no Cassandra key is stated
host     = "127.0.0.1"
port     = 5432
database = "orders"
user     = "ana"
password = { env = "ORDERS_PW" }  # or "literal", or { file = "~/.secrets/orders" }
sslmode  = "verify-full"
posture  = "read-only"            # read-only | writable | protected

[[connections]]
name       = "events"
engine     = "cassandra"          # implied by `hosts`
hosts      = ["10.0.0.1:9042", "10.0.0.2:9042"]
datacenter = "eu-west-1"
user       = "cassandra"
password   = { env = "CASSANDRA_PW" }
tls        = true
```

`engine` may be left out: an entry with `hosts` (or another Cassandra-only
key) is Cassandra, any other is Postgres, so LazyPg and LazyCassandra config
files load unchanged. A key of the other engine drops the entry with a
diagnostic rather than being ignored.

Postgres keys fall back as psql's do (`PG*` variables, then `~/.pgpass`), and
`url` takes a `postgresql://` or JDBC URL. Cassandra connections take
`discovery = "contact-points"` for a tunnel, `cqlshrc` for credentials and
TLS from a cqlsh file, and `consistency` for every request. The
[config reference](crates/ld-outbound/src/config/example.toml) covers every
key, themes, layout, grid, history, driver timeouts and export.

Postures:

- `read-only` (default): no edits, inserts, deletes or drops. Postgres
  sessions also run with `default_transaction_read_only = on`.
- `writable`: writes after a preview; drops and deletes ask `y`/`n`.
- `protected`: writable, but a drop or truncate needs the object's name typed.

Dropping a schema or keyspace always needs its name typed.

## Key bindings

The bottom bar shows what the focused panel takes; `?` lists all of it.

| Keys | Action |
| --- | --- |
| `1` / `2` / `3` / `4` / `5` | Objects / Structure / Table contents / Editor / Logs |
| `Tab` / `Shift-Tab` · `[` / `]` | Next / previous panel · previous / next tab |
| `h` `j` `k` `l` / arrows | Move, fold and unfold |
| `g` / `G` · `Ctrl-u` / `Ctrl-d` | First / last row · page up / down |
| `/` · `n` / `N` | Search the panel · next / previous match |
| `Enter` | Open a table; inspect a row or cell |
| `m` · `v` · `y` | Cycle row / column / cell mode · select rows · copy |
| `s` / `f` / `c` | Sort / filter / clear filter, in column mode, on fetched rows |
| `r` | Re-read the schema in Objects; re-run the read in Table contents |
| `p` / `P` | Fetch the next page / every page |
| `Y` | Copy the selected rows as `INSERT` statements |
| `Ctrl-e` | Export rows; open `$EDITOR` from the editor |
| `Ctrl-r` · `Ctrl-f` · `Ctrl-g` | Run the statement · format the buffer · query history |
| `e` · `i` · `d` | Edit a cell · insert a row · delete rows or drop an object |
| `x` | In the inspector: stage `NULL` |
| `Ctrl-s` | Preview and commit staged edits |
| `Ctrl-o` / `Ctrl-t` | Connection picker / theme picker |
| `Ctrl-c` | Cancel running queries |
| `Esc` / `q` | Back out one step / quit |

In the editor printable keys are text; `Esc` leaves it.

## Project

Pre-1.0. More engines (MySQL, SQL Server, Oracle) are on the
[roadmap](docs/vision.md#6-roadmap). Development:
[docs/development.md](docs/development.md) (fixtures, tasks) and
[docs/architecture.md](docs/architecture.md).

Released under the [0BSD license](LICENSE). The Catppuccin, Solarized and
Dracula themes use their authors' palettes; see the
[third-party notices](THIRD-PARTY-NOTICES.md).
