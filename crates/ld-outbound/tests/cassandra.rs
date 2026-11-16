//! The integration tier against the Cassandra fixture. `mise run db:up`, then
//! `mise run test:it`. `LD_CASSANDRA_PORT` moves the port, as it does the
//! fixture.
#![cfg(feature = "integration")]

use std::collections::BTreeMap;

use ld_domain::Engine;
use ld_domain::config::{
    CassandraTarget, ConnectionConfig, ConnectionName, DriverConfig, PageSize, Target,
};
use ld_domain::model::{
    Catalog, ColumnKind, ConnId, DbError, Page, RelationKind, SchemaKind, SortOrder, Statement,
    Value,
};
use ld_domain::ports::Database;
use ld_domain::posture::Posture;
use ld_outbound::engines::Engines;

#[allow(
    clippy::expect_used,
    reason = "allow-expect-in-tests only covers #[test]-attributed items, not helpers they call"
)]
fn fixture(posture: Posture) -> ConnectionConfig {
    let port = std::env::var("LD_CASSANDRA_PORT").unwrap_or_else(|_| "9043".to_owned());
    ConnectionConfig {
        name: ConnectionName::try_new("cassandra").expect("a valid name"),
        posture,
        target: Target::Cassandra(CassandraTarget::at(&[&format!("127.0.0.1:{port}")])),
    }
}

/// Through the router, as the app reaches it.
#[allow(
    clippy::expect_used,
    reason = "allow-expect-in-tests only covers #[test]-attributed items, not helpers they call"
)]
async fn connected(posture: Posture) -> (Engines, ConnId) {
    let engines = Engines::new();
    let conn = ConnId::next();
    let granted = engines
        .connect(conn, &fixture(posture), &DriverConfig::default())
        .await
        .expect("the fixture is up");
    assert_eq!(granted, posture);
    (engines, conn)
}

#[allow(
    clippy::expect_used,
    reason = "allow-expect-in-tests only covers #[test]-attributed items, not helpers they call"
)]
async fn catalog() -> Catalog {
    let (engines, conn) = connected(Posture::ReadOnly).await;
    engines.describe(conn).await.expect("describes")
}

#[allow(
    clippy::expect_used,
    reason = "allow-expect-in-tests only covers #[test]-attributed items, not helpers they call"
)]
async fn read(cql: &str) -> Page {
    let (engines, conn) = connected(Posture::ReadOnly).await;
    engines
        .execute_paged(
            conn,
            &Statement::typed(Engine::Cassandra, cql),
            PageSize::default(),
            None,
        )
        .await
        .expect("the read succeeds")
}

#[allow(
    clippy::expect_used,
    reason = "allow-expect-in-tests only covers #[test]-attributed items, not helpers they call"
)]
fn row_by_id(page: &Page, id: i64) -> BTreeMap<String, Value> {
    let at = page
        .columns
        .iter()
        .position(|column| column == "id")
        .expect("an id column");
    let row = page
        .rows
        .iter()
        .find(|row| row.values[at] == Value::Int(id))
        .expect("the seeded row");
    page.columns
        .iter()
        .cloned()
        .zip(row.values.iter().cloned())
        .collect()
}

#[tokio::test]
async fn the_catalog_lists_the_seeded_keyspaces_before_the_clusters_own() {
    let catalog = catalog().await;

    assert_eq!(catalog.engine, Engine::Cassandra);
    assert!(catalog.server.is_none(), "a cluster has no databases");
    // Other tests add scratch keyspaces; the seeded ones are what is pinned.
    let names: Vec<_> = catalog
        .schemas_in_display_order()
        .map(|schema| schema.name.as_str())
        .filter(|name| ["edge_cases", "shop", "typezoo", "system"].contains(name))
        .collect();
    assert_eq!(names, ["edge_cases", "shop", "typezoo", "system"]);
    assert_eq!(catalog.schemas["system"].kind(), SchemaKind::System);
}

#[tokio::test]
async fn a_tables_key_keeps_its_partition_and_clustering_order() {
    let catalog = catalog().await;
    let orders = &catalog.schemas["shop"].tables["orders_by_customer"];
    let key = orders.primary_key.as_ref().expect("every table has a key");

    assert_eq!(key.partition().collect::<Vec<_>>(), ["customer_id"]);
    assert_eq!(
        key.clustering().collect::<Vec<_>>(),
        [
            ("order_date", SortOrder::Descending),
            ("order_id", SortOrder::Ascending),
        ]
    );
    assert!(orders.indexes.is_empty());
}

#[tokio::test]
async fn indexes_views_and_static_columns_come_back() {
    let catalog = catalog().await;
    let shop = &catalog.schemas["shop"];

    let customers = &shop.tables["customers_by_id"];
    let targets: Vec<_> = customers
        .indexes
        .iter()
        .map(|index| (index.name.as_str(), index.keys[0].as_str()))
        .collect();
    assert_eq!(
        targets,
        [
            ("customers_by_id_country_code_idx", "country_code"),
            ("customers_by_id_tags_idx", "values(tags)"),
        ]
    );
    assert_eq!(
        customers.column("tags").map(|c| c.data_type.as_str()),
        Some("set<text>")
    );

    let view = &shop.tables["customers_by_email"];
    assert_eq!(view.kind, RelationKind::MaterializedView);
    assert!(
        view.definition
            .as_deref()
            .is_some_and(|select| select.contains("FROM shop.customers_by_id"))
    );

    let statics: Vec<_> = catalog.schemas["typezoo"].tables["static_columns"]
        .columns
        .iter()
        .filter(|column| column.kind == ColumnKind::Static)
        .map(|column| column.name.as_str())
        .collect();
    assert_eq!(statics, ["calibrated_at", "device_model", "firmware"]);
}

#[tokio::test]
async fn every_seeded_table_rebuilds_into_ddl_cassandra_accepts() {
    let catalog = catalog().await;
    let (engines, conn) = connected(Posture::Writable).await;
    engines
        .execute(
            conn,
            &Statement::typed(
                Engine::Cassandra,
                "CREATE KEYSPACE IF NOT EXISTS ddl_check WITH replication = \
                 {'class': 'SimpleStrategy', 'replication_factor': 1}",
            ),
        )
        .await
        .expect("a scratch keyspace");

    for table in catalog.schemas["shop"]
        .tables
        .values()
        .filter(|table| table.kind == RelationKind::Table)
    {
        let renamed = ld_domain::model::Table {
            indexes: Vec::new(),
            ..table.clone()
        };
        let ddl = ld_domain::create::statements(
            Engine::Cassandra,
            &ld_domain::model::Schema::new(Engine::Cassandra, "ddl_check"),
            &renamed,
        )
        .join("\n")
        .replace("CREATE TABLE ", "CREATE TABLE IF NOT EXISTS ");
        let created = engines
            .execute(conn, &Statement::typed(Engine::Cassandra, &ddl))
            .await;
        assert!(created.is_ok(), "{ddl}\n{created:?}");
    }
}

#[tokio::test]
async fn collections_keep_their_shape_and_scalars_read_as_cqlsh_prints_them() {
    let page = read("SELECT * FROM typezoo.collection_types").await;
    let row = row_by_id(&page, 1);
    let text = |column: &str| ld_domain::cell::text(&row[column]);

    assert_eq!(text("list_of_text"), "[alpha, beta, gamma]");
    assert_eq!(text("map_text_int"), "{reads: 1204, writes: 87}");
    assert_eq!(text("frozen_set"), "{1, 2, 3}");
    assert!(
        matches!(&row["map_uuid_text"], Value::Map(entries) if matches!(entries[0].0, Value::Uuid(_)))
    );

    let nulls = read("SELECT * FROM typezoo.nulls_and_empties").await;
    let empty = row_by_id(&nulls, 4);
    assert_eq!(empty["maybe_text"], Value::Text(String::new()));
    assert_eq!(empty["maybe_blob"], Value::Text("0x".to_owned()));
    assert_eq!(row_by_id(&nulls, 2)["maybe_text"], Value::Null);
}

#[tokio::test]
async fn a_scan_pages_by_the_drivers_state() {
    let (engines, conn) = connected(Posture::ReadOnly).await;
    let stmt = Statement::select_all(Engine::Cassandra, "shop", "audit_log_by_day", &[]);
    let size = PageSize::try_new(1).expect("in range");

    let first = engines
        .execute_paged(conn, &stmt, size, None)
        .await
        .expect("a page");
    let second = engines
        .execute_paged(conn, &stmt, size, first.next.as_ref())
        .await
        .expect("a page");

    assert_eq!(first.rows.len(), 1);
    assert!(first.next.is_some());
    assert_eq!(second.rows.len(), 1);
    assert_ne!(first.rows, second.rows, "the cursor moved on");
}

#[tokio::test]
async fn rows_yanked_as_inserts_write_back_and_a_batch_applies_whole() {
    let catalog = catalog().await;
    let (engines, conn) = connected(Posture::Writable).await;
    let table = &catalog.schemas["typezoo"].tables["collection_types"];
    let page = read("SELECT * FROM typezoo.collection_types").await;

    let inserts = ld_domain::yank::inserts(
        Engine::Cassandra,
        "typezoo",
        table,
        &page.columns,
        &page.rows,
    )
    .expect("columns");
    let batch = format!("BEGIN BATCH\n{inserts}\nAPPLY BATCH");
    let written = engines
        .execute(conn, &Statement::typed(Engine::Cassandra, &batch))
        .await;
    assert!(written.is_ok(), "{batch}\n{written:?}");

    let after = read("SELECT * FROM typezoo.collection_types").await;
    assert_eq!(
        after.rows, page.rows,
        "an upsert of the same rows changes none"
    );
}

#[tokio::test]
async fn a_statement_the_cluster_rejects_is_an_error_not_a_panic() {
    let (engines, conn) = connected(Posture::ReadOnly).await;

    let result = engines
        .execute_paged(
            conn,
            &Statement::select_all(Engine::Cassandra, "typezoo", "no_such_table", &[]),
            PageSize::default(),
            None,
        )
        .await;

    assert!(matches!(result, Err(DbError::Invalid(_))), "{result:?}");
    assert_eq!(
        engines.transaction(conn).await,
        Ok(ld_domain::connection::Transaction::Idle)
    );
}
