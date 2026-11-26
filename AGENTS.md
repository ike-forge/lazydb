# Agent instructions — LazyDB

Read before touching code, in this order:

- [docs/vision.md](docs/vision.md) — why, scope, roadmap
- [docs/architecture.md](docs/architecture.md) — crate boundaries, decisions, open questions
- [docs/plan.md](docs/plan.md) — phases, open items, adding an engine
- [docs/conventions.md](docs/conventions.md) — how the code should read

LazyDB merges LazyPg (`../lazypg`) and LazyCassandra (`../lazycassandra`).
Panels speak the shared model in `ld_domain::model`; everything engine-
specific is a `match` on `Engine` (architecture.md §6). Keep a feature only
if every engine has it (vision.md §2).

## Non-negotiables

- **No `unsafe`, no FFI.** Enforced by `unsafe_code = "forbid"`.
- **Push invariants into types.** Newtype over validated `String`, enum over
  `bool`, unrepresentable state over a runtime check.
- **Crate boundaries are load-bearing.** `ld-domain` never depends on
  `tokio`, `ratatui`, `crossterm` or a driver.
- **Zero warnings, zero failing or ignored tests.** `mise run ci` clean before
  calling anything done. Any `#[allow]` is item-scoped with a reason.
- **No narrating comments.** Comments are for edge cases, past bugs, upstream
  defects and engine semantics the code cannot express.
- **Determinism** in the fast tiers: no wall clock, network, filesystem,
  unseeded randomness, `sleep` or `HashMap` order. Time enters as `Msg::Tick`.
- **Git: commit freely, never `pull`.** Push only when asked.
- **Conventional Commits**: `<type>[scope]: <description>`, imperative,
  lowercase, no trailing period.

## Workflow

```
mise run build   # cargo build --workspace
mise run fmt     # cargo fmt --all
mise run check   # fmt, clippy -D warnings, cargo deny
mise run test    # fast tiers
mise run ci      # everything CI gates on
mise run test:it # integration tier, against the fixtures
mise run db:up   # fixture Postgres (5433) and Cassandra (9043), seeded
```
