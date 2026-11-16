//! `pg_catalog`, read per database.

/// System schemas, toast and other sessions' temporary schemas.
macro_rules! user_schemas {
    ($alias:literal) => {
        concat!(
            $alias,
            ".nspname NOT IN ('pg_catalog', 'information_schema') AND ",
            $alias,
            ".nspname !~ '^pg_(toast|temp_)'"
        )
    };
}

/// Templates take no sessions; `datallowconn` and `CONNECT` are what a
/// switch would be refused on.
pub const SERVER: &str = "
    SELECT current_database()::text,
           ARRAY(SELECT datname::text FROM pg_database
                 WHERE datallowconn AND NOT datistemplate
                   AND has_database_privilege(datname, 'CONNECT')
                 ORDER BY 1)";

pub const SCHEMAS: &str = concat!(
    "SELECT n.nspname FROM pg_namespace n WHERE ",
    user_schemas!("n")
);

/// Partitions are their parent's rows, so never listed themselves.
pub const RELATIONS: &str = concat!(
    "SELECT n.nspname, c.relname, c.relkind::text,
            obj_description(c.oid, 'pg_class'),
            CASE WHEN c.relkind IN ('v', 'm') THEN pg_get_viewdef(c.oid) END
     FROM pg_class c
     JOIN pg_namespace n ON n.oid = c.relnamespace
     WHERE c.relkind IN ('r', 'p', 'v', 'm', 'f') AND NOT c.relispartition AND ",
    user_schemas!("n")
);

pub const COLUMNS: &str = concat!(
    "SELECT n.nspname, c.relname, a.attname,
            format_type(a.atttypid, a.atttypmod),
            NOT a.attnotnull,
            a.attidentity::text, a.attgenerated::text,
            pg_get_expr(d.adbin, d.adrelid),
            col_description(c.oid, a.attnum)
     FROM pg_class c
     JOIN pg_namespace n ON n.oid = c.relnamespace
     JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped
     LEFT JOIN pg_attrdef d ON d.adrelid = c.oid AND d.adnum = a.attnum
     WHERE c.relkind IN ('r', 'p', 'v', 'm', 'f') AND NOT c.relispartition AND ",
    user_schemas!("n"),
    " ORDER BY 1, 2, a.attnum"
);

/// Key columns by `conkey` position, which is key order.
pub const CONSTRAINTS: &str = concat!(
    "SELECT n.nspname, c.relname, con.conname, con.contype::text,
            pg_get_constraintdef(con.oid),
            ARRAY(SELECT a.attname::text
                  FROM unnest(con.conkey) WITH ORDINALITY k(attnum, ord)
                  JOIN pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = k.attnum
                  ORDER BY k.ord),
            fn.nspname::text, fc.relname::text,
            ARRAY(SELECT a.attname::text
                  FROM unnest(con.confkey) WITH ORDINALITY k(attnum, ord)
                  JOIN pg_attribute a ON a.attrelid = con.confrelid AND a.attnum = k.attnum
                  ORDER BY k.ord),
            con.confupdtype::text, con.confdeltype::text
     FROM pg_constraint con
     JOIN pg_class c ON c.oid = con.conrelid
     JOIN pg_namespace n ON n.oid = c.relnamespace
     LEFT JOIN pg_class fc ON fc.oid = con.confrelid
     LEFT JOIN pg_namespace fn ON fn.oid = fc.relnamespace
     WHERE con.contype IN ('p', 'f', 'c', 'u', 'x') AND NOT c.relispartition AND ",
    user_schemas!("n"),
    " ORDER BY 1, 2, 3"
);

/// A key is a column name or the expression's text, as `pg_get_indexdef`
/// renders one position.
pub const INDEXES: &str = concat!(
    "SELECT n.nspname, c.relname, i.relname,
            pg_get_indexdef(x.indexrelid),
            ARRAY(SELECT pg_get_indexdef(x.indexrelid, k, true)
                  FROM generate_series(1, x.indnkeyatts) k ORDER BY k),
            x.indisunique, x.indisprimary, am.amname::text,
            pg_get_expr(x.indpred, x.indrelid, true)
     FROM pg_index x
     JOIN pg_class i ON i.oid = x.indexrelid
     JOIN pg_class c ON c.oid = x.indrelid
     JOIN pg_namespace n ON n.oid = c.relnamespace
     JOIN pg_am am ON am.oid = i.relam
     WHERE NOT c.relispartition AND ",
    user_schemas!("n"),
    " ORDER BY 1, 2, 3"
);

/// In the order they are sent, each with what it reads.
pub const ALL: [(&str, &str); 6] = [
    ("databases", SERVER),
    ("schemas", SCHEMAS),
    ("relations", RELATIONS),
    ("columns", COLUMNS),
    ("constraints", CONSTRAINTS),
    ("indexes", INDEXES),
];
