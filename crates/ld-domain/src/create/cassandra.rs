//! Rebuilt from `system_schema`: Cassandra's own `DESCRIBE` is a cqlsh
//! command before 4.0. Table options other than the comment and clustering
//! order are not read.

use super::literal;
use crate::engine::Engine;
use crate::model::{Column, ColumnKind, KeyRole, PrimaryKey, RelationKind, Schema, Table};

/// Bare when CQL would read it back unchanged: lower case, a letter first,
/// not a keyword.
pub(super) fn ident(name: &str) -> String {
    let mut chars = name.chars();
    let plain = chars.next().is_some_and(|first| first.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !crate::sql::is_keyword(Engine::Cassandra, name);

    if plain {
        name.to_owned()
    } else {
        format!("\"{}\"", name.replace('"', "\"\""))
    }
}

pub(super) fn statements(schema: &Schema, table: &Table) -> Vec<String> {
    let name = format!("{}.{}", ident(&schema.name), ident(&table.name));
    let mut lines = match table.kind {
        RelationKind::MaterializedView => view(&name, table),
        RelationKind::Table | RelationKind::View => shell(&name, table),
    };

    let indexes: Vec<String> = table
        .indexes
        .iter()
        .map(|index| format!("{};", index.definition))
        .collect();
    if !indexes.is_empty() {
        lines.push(String::new());
        lines.extend(indexes);
    }
    lines
}

fn shell(name: &str, table: &Table) -> Vec<String> {
    let mut items: Vec<String> = table.columns.iter().map(column).collect();
    if let Some(key) = &table.primary_key {
        items.push(primary_key(key));
    }

    let last = items.len().saturating_sub(1);
    let mut lines = vec![format!("CREATE TABLE {name} (")];
    lines.extend(items.into_iter().enumerate().map(|(at, item)| {
        if at == last {
            format!("    {item}")
        } else {
            format!("    {item},")
        }
    }));
    close(&mut lines, table);
    lines
}

/// The adapter stores the view's `SELECT … WHERE …`; its key closes it.
fn view(name: &str, table: &Table) -> Vec<String> {
    let mut lines = vec![format!("CREATE MATERIALIZED VIEW {name} AS")];
    lines.extend(
        table
            .definition
            .iter()
            .flat_map(|definition| definition.lines())
            .map(|line| format!("    {}", line.trim())),
    );
    if let Some(key) = &table.primary_key {
        lines.push(format!("    {}", primary_key(key)));
    }
    close(&mut lines, table);
    lines
}

/// `)` and the `WITH` clauses on one line each, the last ending the
/// statement.
fn close(lines: &mut Vec<String>, table: &Table) {
    let order: Vec<String> = table
        .primary_key
        .iter()
        .flat_map(PrimaryKey::clustering)
        .map(|(column, order)| format!("{} {}", ident(column), order.keyword()))
        .collect();
    let mut options = Vec::new();
    if !order.is_empty() {
        options.push(format!("CLUSTERING ORDER BY ({})", order.join(", ")));
    }
    if let Some(comment) = &table.comment {
        options.push(format!("comment = {}", literal(comment)));
    }

    let opener = if table.kind == RelationKind::MaterializedView {
        ""
    } else {
        ")"
    };
    match options.split_first() {
        None => lines.push(format!("{opener};")),
        Some((first, rest)) => {
            let mut close = format!("{opener} WITH {first}").trim_start().to_owned();
            if opener.is_empty() {
                close = format!("    {close}");
            }
            lines.push(close);
            let last = rest.len().saturating_sub(1);
            for (at, option) in rest.iter().enumerate() {
                let end = if at == last { ";" } else { "" };
                lines.push(format!("    AND {option}{end}"));
            }
            if rest.is_empty()
                && let Some(line) = lines.last_mut()
            {
                line.push(';');
            }
        }
    }
}

fn column(column: &Column) -> String {
    let mut line = format!("{} {}", ident(&column.name), column.data_type);
    if column.kind == ColumnKind::Static {
        line.push_str(" static");
    }
    line
}

/// `((a, b), c)`: the partition bracketed even alone, so a reader never
/// mistakes a single partition column for a clustering one.
fn primary_key(key: &PrimaryKey) -> String {
    let partition: Vec<String> = key.partition().map(ident).collect();
    let clustering = key
        .columns
        .iter()
        .filter(|column| matches!(column.role, KeyRole::Clustering(_) | KeyRole::Plain))
        .map(|column| ident(&column.name));

    let parts: Vec<String> = std::iter::once(format!("({})", partition.join(", ")))
        .chain(clustering)
        .collect();
    format!("PRIMARY KEY ({})", parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Index, KeyColumn, SortOrder};

    fn readings() -> Table {
        Table {
            name: "readings".to_owned(),
            columns: vec![
                Column::new("device_id", "text"),
                Column::new("reading_at", "timestamp"),
                Column {
                    kind: ColumnKind::Static,
                    ..Column::new("firmware", "text")
                },
                Column::new("Temp", "float"),
            ],
            primary_key: Some(PrimaryKey {
                name: None,
                columns: vec![
                    KeyColumn {
                        name: "device_id".to_owned(),
                        role: KeyRole::Partition,
                    },
                    KeyColumn {
                        name: "reading_at".to_owned(),
                        role: KeyRole::Clustering(SortOrder::Descending),
                    },
                ],
            }),
            indexes: vec![Index {
                name: "readings_by_temp".to_owned(),
                definition: "CREATE INDEX readings_by_temp ON iot.readings (\"Temp\")".to_owned(),
                keys: vec!["Temp".to_owned()],
                unique: false,
                primary: false,
                method: None,
                predicate: None,
            }],
            comment: Some("Sensor data, it's raw.".to_owned()),
            ..Table::default()
        }
    }

    #[test]
    fn a_table_is_rebuilt_with_its_key_shape_static_columns_and_indexes() {
        assert_eq!(
            statements(&Schema::new(Engine::Cassandra, "iot"), &readings()),
            [
                "CREATE TABLE iot.readings (",
                "    device_id text,",
                "    reading_at timestamp,",
                "    firmware text static,",
                "    \"Temp\" float,",
                "    PRIMARY KEY ((device_id), reading_at)",
                ") WITH CLUSTERING ORDER BY (reading_at DESC)",
                "    AND comment = 'Sensor data, it''s raw.';",
                "",
                "CREATE INDEX readings_by_temp ON iot.readings (\"Temp\");",
            ]
        );
    }

    #[test]
    fn a_table_without_options_closes_bare() {
        let table = Table {
            comment: None,
            indexes: Vec::new(),
            primary_key: Some(PrimaryKey {
                name: None,
                columns: vec![KeyColumn {
                    name: "device_id".to_owned(),
                    role: KeyRole::Partition,
                }],
            }),
            ..readings()
        };

        let lines = statements(&Schema::new(Engine::Cassandra, "iot"), &table);
        assert_eq!(
            lines[lines.len() - 2..],
            ["    PRIMARY KEY ((device_id))", ");"]
        );
    }

    #[test]
    fn a_materialized_view_is_its_select_and_its_own_key() {
        let view = Table {
            name: "by_temp".to_owned(),
            kind: RelationKind::MaterializedView,
            definition: Some("SELECT * FROM iot.readings\nWHERE \"Temp\" IS NOT NULL".to_owned()),
            comment: None,
            indexes: Vec::new(),
            ..readings()
        };

        assert_eq!(
            statements(&Schema::new(Engine::Cassandra, "iot"), &view),
            [
                "CREATE MATERIALIZED VIEW iot.by_temp AS",
                "    SELECT * FROM iot.readings",
                "    WHERE \"Temp\" IS NOT NULL",
                "    PRIMARY KEY ((device_id), reading_at)",
                "    WITH CLUSTERING ORDER BY (reading_at DESC);",
            ]
        );
    }

    #[test]
    fn a_name_is_quoted_exactly_when_cql_would_fold_or_refuse_it() {
        assert_eq!(ident("orders"), "orders");
        assert_eq!(ident("Orders"), "\"Orders\"");
        assert_eq!(ident("select"), "\"select\"");
        assert_eq!(ident("_x"), "\"_x\"");
    }
}
