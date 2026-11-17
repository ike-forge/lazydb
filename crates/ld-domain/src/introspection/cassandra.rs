//! `system_schema`, which every keyspace's metadata lives in. Each table is
//! read whole: CQL filters on a partition key, and these are small.

/// `*` here and for tables and views: their option columns differ across
/// majors, so naming them would fail on some clusters.
pub const KEYSPACES: &str = "SELECT * FROM system_schema.keyspaces";

pub const TABLES: &str = "SELECT * FROM system_schema.tables";

/// `position` orders a key's columns; `clustering_order` is `asc`, `desc` or
/// `none`.
pub const COLUMNS: &str = "SELECT keyspace_name, table_name, column_name, kind, position, \
                           clustering_order, type FROM system_schema.columns";

/// `options` holds the `target` and, for a custom index, its `class_name`.
pub const INDEXES: &str =
    "SELECT keyspace_name, table_name, index_name, kind, options FROM system_schema.indexes";

pub const VIEWS: &str = "SELECT * FROM system_schema.views";

pub const ALL: [(&str, &str); 5] = [
    ("keyspaces", KEYSPACES),
    ("tables", TABLES),
    ("columns", COLUMNS),
    ("indexes", INDEXES),
    ("views", VIEWS),
];
