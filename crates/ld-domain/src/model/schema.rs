//! One database's catalog in the vocabulary every engine shares: schemas
//! (Cassandra's keyspaces), their relations, and what keys and references
//! them, made structural so the panels never re-derive it (vision.md §2).

use std::collections::BTreeMap;

use crate::engine::Engine;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Catalog {
    pub engine: Engine,
    /// Where this database sits. `None` for a catalog read on its own, which
    /// the tree roots at its schemas.
    pub server: Option<Server>,
    /// Ordered so iteration is deterministic.
    pub schemas: BTreeMap<String, Schema>,
}

/// The databases a session on this server could switch to. A session is
/// scoped to one, so only `current` has schemas here. `None` on an engine
/// without databases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    pub current: String,
    /// Sorted; includes `current`.
    pub databases: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schema {
    pub name: String,
    pub kind: SchemaKind,
    /// A keyspace's replication and durability; empty for a Postgres schema.
    pub options: Options,
    /// Every relation kind; [`Table::kind`] tells them apart.
    pub tables: BTreeMap<String, Table>,
}

impl Schema {
    #[must_use]
    pub fn new(engine: Engine, name: &str) -> Self {
        Self {
            name: name.to_owned(),
            kind: SchemaKind::of(engine, name),
            options: Options::default(),
            tables: BTreeMap::new(),
        }
    }

    /// A Postgres schema: what most fixtures are.
    #[must_use]
    pub fn empty(name: &str) -> Self {
        Self::new(Engine::Postgres, name)
    }
}

/// The kinds that hold or present rows, in every engine's terms. Postgres's
/// partitioned and foreign tables are tables here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RelationKind {
    #[default]
    Table,
    View,
    MaterializedView,
}

impl RelationKind {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Table => "Table",
            Self::View => "View",
            Self::MaterializedView => "Materialized view",
        }
    }

    /// Only these accept `INSERT`, `UPDATE` and `DELETE` without rules or
    /// triggers.
    #[must_use]
    pub const fn stores_rows(self) -> bool {
        matches!(self, Self::Table)
    }
}

/// A relation of any [`RelationKind`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Table {
    pub name: String,
    pub kind: RelationKind,
    /// What `SELECT *` returns, in its order.
    pub columns: Vec<Column>,
    pub primary_key: Option<PrimaryKey>,
    /// Outgoing; the incoming ones are [`Catalog::referencing`].
    pub foreign_keys: Vec<ForeignKey>,
    pub indexes: Vec<Index>,
    /// Check and unique: the primary and foreign keys have their own fields.
    pub constraints: Vec<Constraint>,
    pub comment: Option<String>,
    /// Empty where the adapter reads none, which is every engine but
    /// Cassandra.
    pub options: Options,
    /// A view's `SELECT` as the server renders it; a Cassandra materialized
    /// view's `SELECT … WHERE`.
    pub definition: Option<String>,
}

/// What a table or schema is configured with beyond its shape, as the server
/// renders each value: Cassandra's table options and a keyspace's
/// replication. Text, not typed fields: the set differs across majors (3.11
/// `read_repair_chance`, 4.0 `read_repair`, 5.0 `memtable`). The ordering is
/// the real knowledge.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Options {
    by_name: BTreeMap<String, String>,
}

/// A layout rule: these fit on one line and read as a block; [`BULKY`] ones
/// wrap.
const RANKED: &[&str] = &[
    "default_time_to_live",
    "gc_grace_seconds",
    "caching",
    "bloom_filter_fp_chance",
    "speculative_retry",
    "additional_write_policy",
    "read_repair",
    "cdc",
    "crc_check_chance",
    "min_index_interval",
    "max_index_interval",
    "memtable",
    "memtable_flush_period_in_ms",
    "allow_auto_snapshot",
    "incremental_backups",
    "flags",
    "id",
    "extensions",
];

/// Below the unranked tail, so an unknown wide option cannot split the block
/// above.
const BULKY: &[&str] = &["comment", "compaction", "compression"];

fn listed(name: &str) -> bool {
    RANKED.contains(&name) || BULKY.contains(&name)
}

impl Options {
    #[must_use]
    pub const fn new(by_name: BTreeMap<String, String>) -> Self {
        Self { by_name }
    }

    /// Unknown options sort alphabetically in the middle, so newer or older
    /// clusters stay fully visible.
    pub fn in_display_order(&self) -> impl Iterator<Item = (&str, &str)> {
        let named = |names: &'static [&'static str]| {
            names
                .iter()
                .filter_map(|name| self.get(name).map(|value| (*name, value)))
        };

        let rest = self
            .by_name
            .iter()
            .filter(|(name, _)| !listed(name))
            .map(|(name, value)| (name.as_str(), value.as_str()));

        named(RANKED).chain(rest).chain(named(BULKY))
    }

    /// These added, replacing any of the same name.
    #[must_use]
    pub fn with(mut self, more: BTreeMap<String, String>) -> Self {
        self.by_name.extend(more);
        self
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.by_name.get(name).map(String::as_str)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub name: String,
    /// As the server renders it, e.g. `character varying(16)` or
    /// `frozen<list<text>>`.
    pub data_type: String,
    pub nullable: bool,
    pub default: Option<ColumnDefault>,
    pub kind: ColumnKind,
    pub comment: Option<String>,
}

/// Three things SQL stores in one place: a default, an identity, a
/// generation expression. They are spelled differently in a `CREATE TABLE`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColumnDefault {
    Expression(String),
    Identity {
        always: bool,
    },
    /// `STORED`: Postgres before 18 has no virtual generated columns.
    Generated(String),
}

impl ColumnDefault {
    /// As it is written after the type in a column definition.
    #[must_use]
    pub fn clause(&self) -> String {
        match self {
            Self::Expression(expression) => format!("DEFAULT {expression}"),
            Self::Identity { always: true } => "GENERATED ALWAYS AS IDENTITY".to_owned(),
            Self::Identity { always: false } => "GENERATED BY DEFAULT AS IDENTITY".to_owned(),
            Self::Generated(expression) => format!("GENERATED ALWAYS AS ({expression}) STORED"),
        }
    }
}

/// `Static`: Cassandra's one value per partition, shared by its rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColumnKind {
    #[default]
    Regular,
    Static,
}

impl Column {
    #[must_use]
    pub fn new(name: &str, data_type: &str) -> Self {
        Self {
            name: name.to_owned(),
            data_type: data_type.to_owned(),
            nullable: true,
            default: None,
            kind: ColumnKind::Regular,
            comment: None,
        }
    }
}

/// The columns that identify a row. SQL's are [`KeyRole::Plain`]; Cassandra's
/// split into the partition, which places a row, and clustering, which orders
/// it within the partition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrimaryKey {
    /// The constraint's name; Cassandra's key has none.
    pub name: Option<String>,
    /// In key order, which need not be column order. Never empty.
    pub columns: Vec<KeyColumn>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyColumn {
    pub name: String,
    pub role: KeyRole,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyRole {
    Plain,
    Partition,
    Clustering(SortOrder),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortOrder {
    #[default]
    Ascending,
    Descending,
}

impl SortOrder {
    #[must_use]
    pub const fn keyword(self) -> &'static str {
        match self {
            Self::Ascending => "ASC",
            Self::Descending => "DESC",
        }
    }
}

impl PrimaryKey {
    /// A SQL key: every column [`KeyRole::Plain`].
    #[must_use]
    pub fn plain<S: AsRef<str>>(name: &str, columns: &[S]) -> Self {
        Self {
            name: Some(name.to_owned()),
            columns: columns
                .iter()
                .map(|column| KeyColumn {
                    name: column.as_ref().to_owned(),
                    role: KeyRole::Plain,
                })
                .collect(),
        }
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.columns.iter().map(|column| column.name.as_str())
    }

    #[must_use]
    pub fn role(&self, column: &str) -> Option<KeyRole> {
        self.columns
            .iter()
            .find(|key| key.name == column)
            .map(|key| key.role)
    }

    pub fn partition(&self) -> impl Iterator<Item = &str> {
        self.columns
            .iter()
            .filter(|key| key.role == KeyRole::Partition)
            .map(|key| key.name.as_str())
    }

    pub fn clustering(&self) -> impl Iterator<Item = (&str, SortOrder)> {
        self.columns.iter().filter_map(|key| match key.role {
            KeyRole::Clustering(order) => Some((key.name.as_str(), order)),
            KeyRole::Plain | KeyRole::Partition => None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignKey {
    pub name: String,
    pub columns: Vec<String>,
    pub target: RelationRef,
    /// Paired with `columns` by position.
    pub target_columns: Vec<String>,
    /// SQL's words: `NO ACTION`, `CASCADE`, …
    pub on_update: String,
    pub on_delete: String,
}

/// Unquoted names.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RelationRef {
    pub schema: String,
    pub table: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Index {
    pub name: String,
    /// The whole `CREATE INDEX`, as the server renders it or the adapter
    /// rebuilds it.
    pub definition: String,
    /// Column names, or the expression text for an expression key, or a
    /// Cassandra target such as `keys(tags)`.
    pub keys: Vec<String>,
    pub unique: bool,
    /// Backs the primary key.
    pub primary: bool,
    /// `btree`, `gin`, or a Cassandra custom index's class. `None` for the
    /// engine's default.
    pub method: Option<String>,
    /// A partial index's `WHERE`.
    pub predicate: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstraintKind {
    Check,
    Unique,
    /// Postgres's `EXCLUDE`.
    Exclusion,
}

impl ConstraintKind {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::Unique => "unique",
            Self::Exclusion => "exclusion",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Constraint {
    pub name: String,
    pub kind: ConstraintKind,
    /// As the server renders it, e.g. `CHECK ((price >= (0)::numeric))`.
    pub definition: String,
}

impl Table {
    pub fn columns(&self) -> impl Iterator<Item = &Column> {
        self.columns.iter()
    }

    #[must_use]
    pub fn column(&self, name: &str) -> Option<&Column> {
        self.columns.iter().find(|column| column.name == name)
    }

    /// Empty without a primary key.
    pub fn key_columns(&self) -> impl Iterator<Item = &Column> {
        self.primary_key
            .iter()
            .flat_map(PrimaryKey::names)
            .filter_map(|name| self.column(name))
    }

    #[must_use]
    pub fn key_role(&self, column: &str) -> Option<KeyRole> {
        self.primary_key.as_ref()?.role(column)
    }

    #[must_use]
    pub fn is_key(&self, column: &str) -> bool {
        self.key_role(column).is_some()
    }

    /// Whether `column` is among some foreign key's columns.
    #[must_use]
    pub fn references(&self, column: &str) -> bool {
        self.foreign_keys
            .iter()
            .any(|key| key.columns.iter().any(|name| name == column))
    }
}

/// Derived from the engine and the name, so an adapter cannot report a
/// contradicting kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum SchemaKind {
    #[default]
    User,
    System,
}

impl SchemaKind {
    /// Postgres: `information_schema` and the `pg_` reserved prefix, which
    /// `CREATE SCHEMA` refuses to users. Cassandra: `system` and `system_*`;
    /// the separator matters, `systematic` is a user's.
    #[must_use]
    pub fn of(engine: Engine, name: &str) -> Self {
        let system = match engine {
            Engine::Postgres => name == "information_schema" || name.starts_with("pg_"),
            Engine::Cassandra => name == "system" || name.starts_with("system_"),
        };
        if system { Self::System } else { Self::User }
    }
}

impl Schema {
    #[must_use]
    pub const fn kind(&self) -> SchemaKind {
        self.kind
    }
}

impl Catalog {
    /// User schemas first, then the system's, each alphabetical.
    pub fn schemas_in_display_order(&self) -> impl Iterator<Item = &Schema> {
        let user = self
            .schemas
            .values()
            .filter(|schema| schema.kind() == SchemaKind::User);
        let system = self
            .schemas
            .values()
            .filter(|schema| schema.kind() == SchemaKind::System);

        user.chain(system)
    }

    /// `User` for a name this catalog does not hold.
    #[must_use]
    pub fn schema_kind(&self, name: &str) -> SchemaKind {
        self.schemas
            .get(name)
            .map_or(SchemaKind::User, Schema::kind)
    }

    #[must_use]
    pub fn table(&self, at: &RelationRef) -> Option<&Table> {
        self.schemas.get(&at.schema)?.tables.get(&at.table)
    }

    /// The foreign keys pointing at `target`, with the relation each is on.
    pub fn referencing<'a>(
        &'a self,
        target: &'a RelationRef,
    ) -> impl Iterator<Item = (RelationRef, &'a ForeignKey)> + 'a {
        self.schemas.values().flat_map(move |schema| {
            schema.tables.values().flat_map(move |table| {
                table
                    .foreign_keys
                    .iter()
                    .filter(move |key| key.target == *target)
                    .map(move |key| {
                        (
                            RelationRef {
                                schema: schema.name.clone(),
                                table: table.name.clone(),
                            },
                            key,
                        )
                    })
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(pairs: &[(&str, &str)]) -> Options {
        Options::new(
            pairs
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect(),
        )
    }

    fn ordered(options: &Options) -> Vec<&str> {
        options.in_display_order().map(|(name, _)| name).collect()
    }

    #[test]
    fn options_are_listed_by_layout_rather_than_by_name() {
        let options = options(&[
            ("bloom_filter_fp_chance", "0.01"),
            ("comment", "orders"),
            ("gc_grace_seconds", "864000"),
        ]);

        assert_eq!(
            ordered(&options),
            ["gc_grace_seconds", "bloom_filter_fp_chance", "comment"]
        );
    }

    #[test]
    fn a_property_this_cassandra_version_does_not_have_is_simply_absent() {
        let options = options(&[("comment", "orders"), ("id", "abc")]);

        assert_eq!(ordered(&options), ["id", "comment"]);
    }

    #[test]
    fn a_property_the_ranking_has_never_heard_of_is_listed_after_the_ranked_ones() {
        // Removed in 4.0: unranked, still shown.
        let options = options(&[
            ("read_repair_chance", "0.0"),
            ("dclocal_read_repair_chance", "0.1"),
            ("comment", "orders"),
            ("id", "abc"),
        ]);

        assert_eq!(
            ordered(&options),
            [
                "id",
                "dclocal_read_repair_chance",
                "read_repair_chance",
                "comment",
            ],
            "unranked properties sort alphabetically after every ranked one, and the bulky ones sort last of all"
        );
    }

    #[test]
    fn a_table_whose_options_row_was_missing_lists_nothing() {
        let options = Options::default();

        assert!(options.is_empty());
        assert_eq!(ordered(&options), [""; 0]);
    }

    #[test]
    fn every_ranked_name_is_listed_once() {
        let mut seen = std::collections::BTreeSet::new();
        for name in RANKED.iter().chain(BULKY.iter()) {
            assert!(
                seen.insert(*name),
                "{name} is ranked twice, so one of the two positions is dead"
            );
        }
    }

    #[test]
    fn the_bulky_options_are_listed_below_even_the_ones_this_build_has_never_heard_of() {
        let options = Options::new(BTreeMap::from([
            ("comment".to_owned(), "Order history.".to_owned()),
            (
                "compaction".to_owned(),
                "{'class': 'SizeTieredCompactionStrategy'}".to_owned(),
            ),
            (
                "compression".to_owned(),
                "{'chunk_length_in_kb': '16'}".to_owned(),
            ),
            ("gc_grace_seconds".to_owned(), "864000".to_owned()),
            ("zzz_option_from_the_future".to_owned(), "on".to_owned()),
        ]));

        assert_eq!(
            options
                .in_display_order()
                .map(|(name, _)| name)
                .collect::<Vec<_>>(),
            [
                "gc_grace_seconds",
                "zzz_option_from_the_future",
                "comment",
                "compaction",
                "compression",
            ],
            "an unranked option must not land between the compact table and the blocks below it"
        );
    }

    fn at(schema: &str, table: &str) -> RelationRef {
        RelationRef {
            schema: schema.to_owned(),
            table: table.to_owned(),
        }
    }

    fn order_line() -> Table {
        Table {
            name: "order_line".to_owned(),
            columns: vec![
                Column::new("order_id", "bigint"),
                Column::new("line_no", "integer"),
                Column::new("customer_id", "bigint"),
            ],
            primary_key: Some(PrimaryKey::plain(
                "order_line_pkey",
                &["line_no", "order_id"],
            )),
            foreign_keys: vec![ForeignKey {
                name: "order_line_customer_id_fkey".to_owned(),
                columns: vec!["customer_id".to_owned()],
                target: at("shop", "customer"),
                target_columns: vec!["id".to_owned()],
                on_update: "NO ACTION".to_owned(),
                on_delete: "CASCADE".to_owned(),
            }],
            ..Table::default()
        }
    }

    fn catalog() -> Catalog {
        let mut shop = Schema::empty("shop");
        shop.tables.insert("order_line".to_owned(), order_line());
        shop.tables.insert(
            "customer".to_owned(),
            Table {
                name: "customer".to_owned(),
                ..Table::default()
            },
        );
        let mut catalog = Catalog::default();
        catalog.schemas.insert("shop".to_owned(), shop);
        catalog
            .schemas
            .insert("pg_catalog".to_owned(), Schema::empty("pg_catalog"));
        catalog
            .schemas
            .insert("public".to_owned(), Schema::empty("public"));
        catalog
    }

    #[test]
    fn key_columns_follow_key_order_not_column_order() {
        let names: Vec<_> = order_line().key_columns().map(|c| c.name.clone()).collect();
        assert_eq!(names, ["line_no", "order_id"]);
    }

    #[test]
    fn a_keyless_table_has_no_key_columns_and_no_column_is_key() {
        let keyless = Table {
            primary_key: None,
            ..order_line()
        };
        assert_eq!(keyless.key_columns().count(), 0);
        assert!(!keyless.is_key("order_id"));
    }

    #[test]
    fn a_column_is_key_or_references_by_the_constraints_it_is_in() {
        let table = order_line();
        assert!(table.is_key("order_id"));
        assert!(!table.is_key("customer_id"));
        assert!(table.references("customer_id"));
        assert!(!table.references("line_no"));
    }

    #[test]
    fn the_foreign_keys_into_a_table_are_found_from_the_other_side() {
        let catalog = catalog();
        let target = at("shop", "customer");
        let incoming: Vec<_> = catalog
            .referencing(&target)
            .map(|(from, key)| (from.table, key.name.clone()))
            .collect();

        assert_eq!(
            incoming,
            [(
                "order_line".to_owned(),
                "order_line_customer_id_fkey".to_owned()
            )]
        );
        assert_eq!(catalog.referencing(&at("shop", "order_line")).count(), 0);
    }

    #[test]
    fn the_reserved_prefix_and_information_schema_are_the_systems() {
        let of = |name| SchemaKind::of(Engine::Postgres, name);
        assert_eq!(of("pg_catalog"), SchemaKind::System);
        assert_eq!(of("pg_toast"), SchemaKind::System);
        assert_eq!(of("information_schema"), SchemaKind::System);
        assert_eq!(of("public"), SchemaKind::User);
        assert_eq!(of("pgx"), SchemaKind::User);
        assert_eq!(
            of("system"),
            SchemaKind::User,
            "a Postgres user may name one so"
        );
    }

    #[test]
    fn the_clusters_own_keyspaces_are_system_and_the_system_prefixed_ones() {
        let of = |name| SchemaKind::of(Engine::Cassandra, name);
        assert_eq!(of("system"), SchemaKind::System);
        assert_eq!(of("system_schema"), SchemaKind::System);
        assert_eq!(of("systematic"), SchemaKind::User);
        assert_eq!(of("pg_catalog"), SchemaKind::User);
    }

    #[test]
    fn a_cassandra_key_tells_partition_from_clustering() {
        let key = PrimaryKey {
            name: None,
            columns: vec![
                KeyColumn {
                    name: "device".to_owned(),
                    role: KeyRole::Partition,
                },
                KeyColumn {
                    name: "at".to_owned(),
                    role: KeyRole::Clustering(SortOrder::Descending),
                },
            ],
        };
        assert_eq!(key.partition().collect::<Vec<_>>(), ["device"]);
        assert_eq!(
            key.clustering().collect::<Vec<_>>(),
            [("at", SortOrder::Descending)]
        );
        assert_eq!(key.names().collect::<Vec<_>>(), ["device", "at"]);
    }

    #[test]
    fn display_order_puts_user_schemas_before_the_systems() {
        let names: Vec<_> = catalog()
            .schemas_in_display_order()
            .map(|schema| schema.name.clone())
            .collect();
        assert_eq!(names, ["public", "shop", "pg_catalog"]);
    }

    #[test]
    fn a_relation_is_found_by_reference() {
        assert!(catalog().table(&at("shop", "customer")).is_some());
        assert!(catalog().table(&at("shop", "nope")).is_none());
    }
}
