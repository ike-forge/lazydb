//! The `Database` port over `scylla`. Driver types stop here (architecture.md
//! §5). The catalog is read from `system_schema` with the queries
//! [`ld_domain::introspection::cassandra`] holds, so the Logs tab shows what was
//! asked.

use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use ld_domain::Engine;
use ld_domain::config::{
    CassandraTarget, ConnectionConfig, Consistency, Discovery, DriverConfig, PageSize, Target, Tls,
};
use ld_domain::connection::Transaction;
use ld_domain::introspection::cassandra::{COLUMNS, INDEXES, KEYSPACES, TABLES, VIEWS};
use ld_domain::model::{
    Catalog, Column, ColumnKind, ConnId, DbError, Index, KeyColumn, KeyRole, Options, Page,
    PagingState, PrimaryKey, RelationKind, Row, Schema, SortOrder, Statement, Table, Value,
};
use ld_domain::ports::Database;
use ld_domain::posture::Posture;
use scylla::client::execution_profile::ExecutionProfile;
use scylla::client::session::Session;
use scylla::client::session_builder::SessionBuilder;
use scylla::errors::TranslationError;
use scylla::policies::address_translator::{AddressTranslator, UntranslatedPeer};
use scylla::policies::load_balancing::DefaultPolicy;
use scylla::value::Row as DriverRow;
use tokio::sync::RwLock;

use crate::secret::Secrets;
use crate::tls;
use crate::value::to_value;

#[derive(Debug, Default)]
pub struct Cassandra {
    open: RwLock<HashMap<ConnId, Arc<Session>>>,
}

/// A host filter cannot do this: every peer, including the control connection's
/// node, is `NodeAddr::Translatable`, so a filter narrow enough rejects the
/// tunnelled node too. The driver then pools every peer to the one socket,
/// which is the price of a single tunnel.
#[derive(Debug)]
struct ToTunnel(SocketAddr);

#[async_trait]
impl AddressTranslator for ToTunnel {
    async fn translate_address(
        &self,
        _peer: &UntranslatedPeer,
    ) -> Result<SocketAddr, TranslationError> {
        Ok(self.0)
    }
}

/// Resolved here: a tunnel is as likely named `localhost` as written as an
/// address.
async fn tunnel(target: &CassandraTarget) -> Result<SocketAddr, DbError> {
    let [host] = target.hosts.as_slice() else {
        return Err(DbError::Unreachable(
            "a tunnelled connection reaches exactly one host".to_owned(),
        ));
    };

    tokio::net::lookup_host(host)
        .await
        .map_err(|error| DbError::Unreachable(format!("{host}: {error}")))?
        .next()
        .ok_or_else(|| DbError::Unreachable(format!("{host} resolved to no address")))
}

/// By message: the server's rejection has no `source`, and rustls causes are
/// skipped by `Error::source`.
fn connect_error(message: &str, plaintext: bool) -> DbError {
    const AUTH: [&str; 2] = ["Authentication failed", "Authentication is required"];
    const TLS: [&str; 5] = [
        "invalid peer certificate",
        "received fatal alert",
        "received corrupt message",
        "peer is incompatible",
        "peer misbehaved",
    ];

    if AUTH.iter().any(|marker| message.contains(marker)) {
        DbError::Authentication(message.to_owned())
    } else if TLS.iter().any(|marker| message.contains(marker)) {
        DbError::Tls(message.to_owned())
    } else if plaintext && message.contains("early eof") {
        // A TLS-only node closes a plaintext handshake without a reply.
        DbError::Unreachable(format!("{message} — does the cluster require tls?"))
    } else {
        DbError::Unreachable(message.to_owned())
    }
}

fn request_error(error: &impl std::fmt::Display) -> DbError {
    DbError::Invalid(error.to_string())
}

fn consistency(consistency: Consistency) -> scylla::statement::Consistency {
    use scylla::statement::Consistency as Driver;

    match consistency {
        Consistency::Any => Driver::Any,
        Consistency::One => Driver::One,
        Consistency::Two => Driver::Two,
        Consistency::Three => Driver::Three,
        Consistency::Quorum => Driver::Quorum,
        Consistency::All => Driver::All,
        Consistency::LocalQuorum => Driver::LocalQuorum,
        Consistency::EachQuorum => Driver::EachQuorum,
        Consistency::LocalOne => Driver::LocalOne,
        Consistency::Serial => Driver::Serial,
        Consistency::LocalSerial => Driver::LocalSerial,
    }
}

/// With a datacenter stated, failover elsewhere is refused: silently reading
/// across regions is worse than an error.
fn execution_profile(target: &CassandraTarget, driver: &DriverConfig) -> ExecutionProfile {
    let mut policy = DefaultPolicy::builder();

    if let Some(datacenter) = &target.datacenter {
        policy = policy
            .prefer_datacenter(datacenter.clone())
            .permit_dc_failover(false);
    }

    ExecutionProfile::builder()
        .consistency(consistency(target.consistency))
        .request_timeout(Some(Duration::from_millis(
            driver.request_timeout.get().into(),
        )))
        .load_balancing_policy(policy.build())
        .build()
}

impl Cassandra {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    async fn session(&self, conn: ConnId) -> Result<Arc<Session>, DbError> {
        self.open
            .read()
            .await
            .get(&conn)
            .cloned()
            .ok_or_else(|| DbError::Unreachable("not connected".to_owned()))
    }
}

#[async_trait]
impl Database for Cassandra {
    async fn connect(
        &self,
        conn: ConnId,
        entry: &ConnectionConfig,
        driver: &DriverConfig,
    ) -> Result<Posture, DbError> {
        let Target::Cassandra(target) = &entry.target else {
            return Err(DbError::Unreachable(format!(
                "{} is not a Cassandra connection",
                entry.name.as_str()
            )));
        };
        let mut builder = SessionBuilder::new()
            .known_nodes(&target.hosts)
            .connection_timeout(Duration::from_millis(driver.connect_timeout.get().into()))
            .default_execution_profile_handle(execution_profile(target, driver).into_handle());

        let tls = match &target.tls {
            Tls::Cqlshrc(path) => Secrets
                .cqlshrc_tls(path)
                .await
                .map_err(|error| DbError::Unreachable(error.to_string()))?,
            other => other.clone(),
        };
        if let Tls::On(settings) = &tls {
            builder = builder.tls_context(Some(tls::client_config(settings).await?));
        }

        if let Some(credentials) = &target.credentials {
            // Resolved here so the plaintext never enters `ld-domain`.
            let (username, password) = Secrets
                .credentials(credentials)
                .await
                .map_err(|error| DbError::Unreachable(error.to_string()))?;
            builder = builder.user(username, password);
        }

        if target.discovery == Discovery::ContactPoints {
            builder = builder.address_translator(Arc::new(ToTunnel(tunnel(target).await?)));
        }

        // Boxed for `clippy::large_futures`.
        let session = Box::pin(builder.build())
            .await
            .map_err(|error| connect_error(&error.to_string(), tls == Tls::Off))?;

        let mut open = self.open.write().await;
        // One session at a time: a reconnect replaces the last.
        open.clear();
        open.insert(conn, Arc::new(session));

        Ok(entry.posture)
    }

    async fn describe(&self, conn: ConnId) -> Result<Catalog, DbError> {
        let session = self.session(conn).await?;
        let read = |cql: &'static str| {
            let session = Arc::clone(&session);
            async move {
                session
                    .query_unpaged(cql, &[])
                    .await
                    .map_err(|error| DbError::Other(error.to_string()))?
                    .into_rows_result()
                    .map_err(|error| DbError::Other(error.to_string()))
            }
        };
        let (keyspaces, tables, columns, indexes, views) = tokio::try_join!(
            read(KEYSPACES),
            read(TABLES),
            read(COLUMNS),
            read(INDEXES),
            read(VIEWS),
        )?;
        let other = |error: scylla::errors::DeserializationError| DbError::Other(error.to_string());
        let collect = |error: scylla::errors::RowsError| DbError::Other(error.to_string());

        let rows = Rows {
            keyspaces: named(&keyspaces)?,
            tables: named(&tables)?,
            columns: columns
                .rows::<(String, String, String, String, i32, String, String)>()
                .map_err(collect)?
                .map(|row| {
                    row.map(
                        |(keyspace, table, name, kind, position, order, data_type)| ColumnRow {
                            keyspace,
                            table,
                            name,
                            kind,
                            position,
                            order,
                            data_type,
                        },
                    )
                    .map_err(other)
                })
                .collect::<Result<_, _>>()?,
            // A null map deserialises to `None`; such an index is still listed.
            indexes: indexes
                .rows::<(
                    String,
                    String,
                    String,
                    String,
                    Option<BTreeMap<String, String>>,
                )>()
                .map_err(collect)?
                .map(|row| {
                    row.map(|(keyspace, table, name, kind, options)| IndexRow {
                        keyspace,
                        table,
                        name,
                        kind,
                        options: options.unwrap_or_default(),
                    })
                    .map_err(other)
                })
                .collect::<Result<_, _>>()?,
            views: named(&views)?,
        };

        Ok(catalog(rows))
    }

    /// No transaction to report: a batch is one request.
    async fn transaction(&self, _conn: ConnId) -> Result<Transaction, DbError> {
        Ok(Transaction::Idle)
    }

    /// Unpaged: only mutations and DDL come here.
    async fn execute(&self, conn: ConnId, stmt: &Statement) -> Result<Page, DbError> {
        let session = self.session(conn).await?;
        let result = session
            .query_unpaged(stmt.sql.as_str(), &[])
            .await
            .map_err(|error| request_error(&error))?;

        to_page(result)
    }

    async fn execute_paged(
        &self,
        conn: ConnId,
        stmt: &Statement,
        page_size: PageSize,
        cursor: Option<&PagingState>,
    ) -> Result<Page, DbError> {
        let session = self.session(conn).await?;
        // scylla panics on a non-positive page size.
        let statement = scylla::statement::unprepared::Statement::new(stmt.sql.as_str())
            .with_page_size(i32::try_from(page_size.get()).unwrap_or(i32::MAX));
        let paging = cursor.map_or(
            scylla::response::PagingState::start(),
            |PagingState(bytes)| scylla::response::PagingState::new_from_raw_bytes(bytes.clone()),
        );

        let (result, response) = session
            .query_single_page(statement, &[], paging)
            .await
            .map_err(|error| request_error(&error))?;

        let mut page = to_page(result)?;
        page.next = match response.into_paging_control_flow() {
            std::ops::ControlFlow::Continue(state) => state
                .as_bytes_slice()
                .map(|bytes| PagingState(bytes.to_vec())),
            std::ops::ControlFlow::Break(()) => None,
        };

        Ok(page)
    }
}

/// A `SELECT *` row by column name: the option columns differ across majors,
/// so naming them would fail on some clusters.
fn named(result: &scylla::response::query_result::QueryRowsResult) -> Result<Vec<Named>, DbError> {
    let names: Vec<String> = result
        .column_specs()
        .iter()
        .map(|spec| spec.name().to_owned())
        .collect();
    result
        .rows::<DriverRow>()
        .map_err(|error| DbError::Other(error.to_string()))?
        .map(|row| {
            let row = row.map_err(|error| DbError::Other(error.to_string()))?;
            Ok(names
                .iter()
                .cloned()
                .zip(row.columns.iter().map(|value| to_value(value.as_ref())))
                .collect())
        })
        .collect()
}

fn to_page(result: scylla::response::query_result::QueryResult) -> Result<Page, DbError> {
    use scylla::response::query_result::IntoRowsResultError;

    let result = match result.into_rows_result() {
        Ok(result) => result,
        // The driver reports a `Void` result as a failed downcast; it is an
        // empty page, not a rejection.
        Err(IntoRowsResultError::ResultNotRows(_)) => return Ok(Page::default()),
        Err(error) => return Err(DbError::Invalid(error.to_string())),
    };

    let columns = result
        .column_specs()
        .iter()
        .map(|spec| spec.name().to_owned())
        .collect();

    let rows = result
        .rows::<DriverRow>()
        .map_err(|error| DbError::Invalid(error.to_string()))?
        .map(|row| {
            row.map(|row| Row {
                values: row.columns.iter().map(|v| to_value(v.as_ref())).collect(),
            })
            .map_err(|error| DbError::Invalid(error.to_string()))
        })
        .collect::<Result<Vec<_>, DbError>>()?;

    Ok(Page {
        columns,
        rows,
        next: None,
    })
}

type Named = BTreeMap<String, Value>;

/// `system_schema`'s rows, as read. Kept apart from the session so the
/// mapping tests without a cluster.
#[derive(Debug, Default)]
struct Rows {
    keyspaces: Vec<Named>,
    tables: Vec<Named>,
    columns: Vec<ColumnRow>,
    indexes: Vec<IndexRow>,
    views: Vec<Named>,
}

fn text(row: &Named, column: &str) -> String {
    match row.get(column) {
        Some(Value::Text(text)) => text.clone(),
        _ => String::new(),
    }
}

/// Every column but the ones that name the object or are drawn elsewhere, as
/// the grid renders a cell. An empty string draws as `''`, or `speculative_retry`
/// reads as broken.
fn options(row: &Named, skipped: &[&str]) -> Options {
    Options::new(
        row.iter()
            .filter(|(name, _)| !skipped.contains(&name.as_str()))
            .map(|(name, value)| {
                let text = ld_domain::cell::text(value);
                (
                    name.clone(),
                    if text.is_empty() {
                        "''".to_owned()
                    } else {
                        text
                    },
                )
            })
            .collect(),
    )
}

/// Replication's map spread into rows, as cqlsh's `DESCRIBE` reads it: the
/// strategy by its short name, then each factor.
fn keyspace_options(row: &Named) -> Options {
    let mut options = options(row, &["keyspace_name", "replication"]);
    if let Some(Value::Map(entries)) = row.get("replication") {
        let mut by_name = std::collections::BTreeMap::new();
        for (key, value) in entries {
            let key = ld_domain::cell::text(key);
            let value = ld_domain::cell::text(value);
            let value = if key == "class" {
                value.rsplit('.').next().unwrap_or(&value).to_owned()
            } else {
                value
            };
            by_name.insert(format!("replication.{key}"), value);
        }
        options = options.with(by_name);
    }
    options
}

fn comment(row: &Named) -> Option<String> {
    Some(text(row, "comment")).filter(|comment| !comment.is_empty())
}

#[derive(Debug)]
struct ColumnRow {
    keyspace: String,
    table: String,
    name: String,
    /// `partition_key`, `clustering`, `static` or `regular`.
    kind: String,
    /// Within the key; `-1` for the rest.
    position: i32,
    order: String,
    data_type: String,
}

#[derive(Debug)]
struct IndexRow {
    keyspace: String,
    table: String,
    name: String,
    /// `COMPOSITES`, `KEYS` or `CUSTOM`.
    kind: String,
    options: BTreeMap<String, String>,
}

struct View {
    keyspace: String,
    name: String,
    base: String,
    filter: Option<String>,
}

/// A table whose columns raced a DDL change between the reads has no key, and
/// is listed keyless rather than dropping the whole catalog.
fn catalog(rows: Rows) -> Catalog {
    let mut schemas: BTreeMap<String, Schema> = rows
        .keyspaces
        .iter()
        .map(|row| {
            let name = text(row, "keyspace_name");
            let schema = Schema {
                options: keyspace_options(row),
                ..Schema::new(Engine::Cassandra, &name)
            };
            (name, schema)
        })
        .collect();

    let mut columns: BTreeMap<(String, String), Vec<ColumnRow>> = BTreeMap::new();
    for row in rows.columns {
        columns
            .entry((row.keyspace.clone(), row.table.clone()))
            .or_default()
            .push(row);
    }
    let mut indexes: BTreeMap<(String, String), Vec<IndexRow>> = BTreeMap::new();
    for row in rows.indexes {
        indexes
            .entry((row.keyspace.clone(), row.table.clone()))
            .or_default()
            .push(row);
    }

    for row in rows.tables {
        let key = (text(&row, "keyspace_name"), text(&row, "table_name"));
        let table = Table {
            name: key.1.clone(),
            comment: comment(&row),
            options: options(&row, &["keyspace_name", "table_name", "comment"]),
            indexes: indexes
                .remove(&key)
                .unwrap_or_default()
                .into_iter()
                .map(|row| index(&key.0, &key.1, row))
                .collect(),
            ..shape(columns.get(&key).map_or(&[][..], Vec::as_slice))
        };
        if let Some(schema) = schemas.get_mut(&key.0) {
            schema.tables.insert(key.1, table);
        }
    }

    for row in rows.views {
        let view = View {
            keyspace: text(&row, "keyspace_name"),
            name: text(&row, "view_name"),
            base: text(&row, "base_table_name"),
            filter: Some(text(&row, "where_clause")),
        };
        let key = (view.keyspace.clone(), view.name.clone());
        let shaped = shape(columns.get(&key).map_or(&[][..], Vec::as_slice));
        let selected: Vec<String> = shaped
            .columns
            .iter()
            .map(|column| ld_domain::create::ident(Engine::Cassandra, &column.name))
            .collect();
        let mut definition = format!(
            "SELECT {} FROM {}.{}",
            selected.join(", "),
            ld_domain::create::ident(Engine::Cassandra, &view.keyspace),
            ld_domain::create::ident(Engine::Cassandra, &view.base)
        );
        if let Some(filter) = view.filter.filter(|filter| !filter.is_empty()) {
            definition = format!("{definition}\nWHERE {filter}");
        }
        let table = Table {
            name: view.name.clone(),
            kind: RelationKind::MaterializedView,
            comment: comment(&row),
            options: options(
                &row,
                &[
                    "keyspace_name",
                    "view_name",
                    "base_table_name",
                    "base_table_id",
                    "where_clause",
                    "include_all_columns",
                    "comment",
                ],
            ),
            definition: Some(definition),
            ..shaped
        };
        if let Some(schema) = schemas.get_mut(&view.keyspace) {
            schema.tables.insert(view.name, table);
        }
    }

    Catalog {
        engine: Engine::Cassandra,
        server: None,
        schemas,
    }
}

/// The key in its own order, then static and regular columns by name:
/// Cassandra keeps no declaration order for them, and `SELECT *` returns
/// this one.
fn shape(rows: &[ColumnRow]) -> Table {
    let keyed = |kind: &str| {
        let mut keyed: Vec<&ColumnRow> = rows.iter().filter(|row| row.kind == kind).collect();
        keyed.sort_by_key(|row| row.position);
        keyed
    };
    let partition = keyed("partition_key");
    let clustering = keyed("clustering");
    let mut rest: Vec<&ColumnRow> = rows
        .iter()
        .filter(|row| matches!(row.kind.as_str(), "static" | "regular"))
        .collect();
    rest.sort_by(|left, right| left.name.cmp(&right.name));

    let key: Vec<KeyColumn> = partition
        .iter()
        .map(|row| KeyColumn {
            name: row.name.clone(),
            role: KeyRole::Partition,
        })
        .chain(clustering.iter().map(|row| KeyColumn {
            name: row.name.clone(),
            role: KeyRole::Clustering(order(&row.order)),
        }))
        .collect();

    let column = |row: &&ColumnRow| Column {
        // A key column is never null.
        nullable: !matches!(row.kind.as_str(), "partition_key" | "clustering"),
        kind: if row.kind == "static" {
            ColumnKind::Static
        } else {
            ColumnKind::Regular
        },
        ..Column::new(&row.name, &row.data_type)
    };

    Table {
        columns: partition
            .iter()
            .chain(&clustering)
            .chain(&rest)
            .map(column)
            .collect(),
        primary_key: (!partition.is_empty()).then_some(PrimaryKey {
            name: None,
            columns: key,
        }),
        ..Table::default()
    }
}

/// `none` reaches a key column only mid-upgrade, where ascending is the
/// default.
fn order(order: &str) -> SortOrder {
    if order.eq_ignore_ascii_case("desc") {
        SortOrder::Descending
    } else {
        SortOrder::Ascending
    }
}

/// The target as Cassandra stores it: `"Status"`, or `keys(tags)` for part of
/// a collection. A plain column is unquoted, so its type can be found.
fn index(keyspace: &str, table: &str, row: IndexRow) -> Index {
    let target = row.options.get("target").cloned().unwrap_or_default();
    let class = row
        .options
        .get("class_name")
        .filter(|_| row.kind.eq_ignore_ascii_case("custom"))
        .cloned();
    let on = format!(
        "{}.{}",
        ld_domain::create::ident(Engine::Cassandra, keyspace),
        ld_domain::create::ident(Engine::Cassandra, table)
    );
    let definition = match &class {
        Some(class) => format!(
            "CREATE CUSTOM INDEX {} ON {on} ({target}) USING '{class}'",
            ld_domain::create::ident(Engine::Cassandra, &row.name)
        ),
        None => format!(
            "CREATE INDEX {} ON {on} ({target})",
            ld_domain::create::ident(Engine::Cassandra, &row.name)
        ),
    };
    let key = target
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .map_or_else(|| target.clone(), |name| name.replace("\"\"", "\""));

    Index {
        name: row.name,
        definition,
        keys: vec![key],
        unique: false,
        primary: false,
        // The class's last segment: `StorageAttachedIndex`, `SASIIndex`.
        method: class.map(|class| {
            class
                .rsplit('.')
                .next()
                .map_or_else(|| class.clone(), str::to_owned)
        }),
        predicate: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(table: &str, name: &str, kind: &str, position: i32, data_type: &str) -> ColumnRow {
        ColumnRow {
            keyspace: "iot".to_owned(),
            table: table.to_owned(),
            name: name.to_owned(),
            kind: kind.to_owned(),
            position,
            order: if name == "at" { "desc" } else { "none" }.to_owned(),
            data_type: data_type.to_owned(),
        }
    }

    fn named(pairs: &[(&str, &str)]) -> Named {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), Value::Text((*value).to_owned())))
            .collect()
    }

    #[test]
    fn options_are_every_column_but_the_names_and_the_comment() {
        let catalog = catalog(readings());
        let options = &catalog.schemas["iot"].tables["readings"].options;

        assert_eq!(
            options.in_display_order().collect::<Vec<_>>(),
            [("gc_grace_seconds", "864000"), ("speculative_retry", "''")]
        );
        assert_eq!(
            catalog.schemas["iot"]
                .options
                .in_display_order()
                .collect::<Vec<_>>(),
            [
                ("replication.class", "SimpleStrategy"),
                ("replication.replication_factor", "1"),
            ]
        );
    }

    fn readings() -> Rows {
        Rows {
            keyspaces: vec![
                {
                    let mut iot = named(&[("keyspace_name", "iot")]);
                    iot.insert(
                        "replication".to_owned(),
                        Value::Map(vec![
                            (
                                Value::Text("class".to_owned()),
                                Value::Text(
                                    "org.apache.cassandra.locator.SimpleStrategy".to_owned(),
                                ),
                            ),
                            (
                                Value::Text("replication_factor".to_owned()),
                                Value::Text("1".to_owned()),
                            ),
                        ]),
                    );
                    iot
                },
                named(&[("keyspace_name", "system")]),
            ],
            tables: vec![named(&[
                ("keyspace_name", "iot"),
                ("table_name", "readings"),
                ("comment", ""),
                ("gc_grace_seconds", "864000"),
                ("speculative_retry", ""),
            ])],
            columns: vec![
                column("readings", "temp", "regular", -1, "float"),
                column("readings", "at", "clustering", 0, "timestamp"),
                column("readings", "site", "partition_key", 1, "text"),
                column("readings", "device", "partition_key", 0, "text"),
                column("readings", "firmware", "static", -1, "text"),
                column("by_temp", "temp", "partition_key", 0, "float"),
                column("by_temp", "device", "clustering", 0, "text"),
            ],
            indexes: vec![IndexRow {
                keyspace: "iot".to_owned(),
                table: "readings".to_owned(),
                name: "readings_sai".to_owned(),
                kind: "CUSTOM".to_owned(),
                options: BTreeMap::from([
                    ("target".to_owned(), "\"Temp\"".to_owned()),
                    (
                        "class_name".to_owned(),
                        "org.apache.cassandra.index.sai.StorageAttachedIndex".to_owned(),
                    ),
                ]),
            }],
            views: vec![named(&[
                ("keyspace_name", "iot"),
                ("view_name", "by_temp"),
                ("base_table_name", "readings"),
                ("where_clause", "temp IS NOT NULL"),
                ("comment", ""),
            ])],
        }
    }

    #[test]
    fn the_key_keeps_its_own_order_and_the_rest_sort_by_name() {
        let catalog = catalog(readings());
        let table = &catalog.schemas["iot"].tables["readings"];

        let names: Vec<_> = table.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["device", "site", "at", "firmware", "temp"]);
        let key = table.primary_key.as_ref().expect("a key");
        assert_eq!(key.partition().collect::<Vec<_>>(), ["device", "site"]);
        assert_eq!(
            key.clustering().collect::<Vec<_>>(),
            [("at", SortOrder::Descending)]
        );
        assert_eq!(table.columns[3].kind, ColumnKind::Static);
        assert!(!table.columns[0].nullable && table.columns[4].nullable);
        assert_eq!(table.comment, None, "an empty comment is none");
    }

    #[test]
    fn the_clusters_keyspaces_are_system_and_there_is_no_server_level() {
        let catalog = catalog(readings());

        assert_eq!(catalog.engine, Engine::Cassandra);
        assert!(catalog.server.is_none());
        assert_eq!(
            catalog.schemas["system"].kind(),
            ld_domain::model::SchemaKind::System
        );
    }

    #[test]
    fn a_custom_index_names_its_class_and_rebuilds_its_statement() {
        let catalog = catalog(readings());
        let index = &catalog.schemas["iot"].tables["readings"].indexes[0];

        assert_eq!(index.keys, ["Temp"], "the column, unquoted");
        assert_eq!(index.method.as_deref(), Some("StorageAttachedIndex"));
        assert_eq!(
            index.definition,
            "CREATE CUSTOM INDEX readings_sai ON iot.readings (\"Temp\") \
             USING 'org.apache.cassandra.index.sai.StorageAttachedIndex'"
        );
    }

    #[test]
    fn a_materialized_view_is_its_select_over_the_base_table() {
        let catalog = catalog(readings());
        let view = &catalog.schemas["iot"].tables["by_temp"];

        assert_eq!(view.kind, RelationKind::MaterializedView);
        assert_eq!(
            view.definition.as_deref(),
            Some("SELECT temp, device FROM iot.readings\nWHERE temp IS NOT NULL")
        );
        assert_eq!(
            view.primary_key
                .as_ref()
                .map(|key| key.names().collect::<Vec<_>>()),
            Some(vec!["temp", "device"])
        );
    }

    #[test]
    fn a_rejected_login_is_an_authentication_failure_and_tls_trouble_is_named() {
        assert!(matches!(
            connect_error("Authentication failed: bad credentials", false),
            DbError::Authentication(_)
        ));
        assert!(matches!(
            connect_error("invalid peer certificate: UnknownIssuer", false),
            DbError::Tls(_)
        ));
        assert!(matches!(
            connect_error("early eof", true),
            DbError::Unreachable(message) if message.contains("tls")
        ));
    }
}
