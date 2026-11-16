//! One `Database` over every engine: each connection goes to the adapter its
//! entry names, and later requests follow its `ConnId`.

use std::collections::HashMap;

use async_trait::async_trait;
use ld_domain::Engine;
use ld_domain::config::{ConnectionConfig, DriverConfig, PageSize};
use ld_domain::connection::Transaction;
use ld_domain::model::{Catalog, ConnId, DbError, Page, PagingState, Statement};
use ld_domain::ports::Database;
use ld_domain::posture::Posture;
use tokio::sync::RwLock;

use crate::cassandra::Cassandra;
use crate::postgres::Postgres;

#[derive(Debug, Default)]
pub struct Engines {
    postgres: Postgres,
    cassandra: Cassandra,
    routes: RwLock<HashMap<ConnId, Engine>>,
}

impl Engines {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    const fn adapter(&self, engine: Engine) -> &dyn Database {
        match engine {
            Engine::Postgres => &self.postgres,
            Engine::Cassandra => &self.cassandra,
        }
    }

    async fn route(&self, conn: ConnId) -> Result<&dyn Database, DbError> {
        let engine = self
            .routes
            .read()
            .await
            .get(&conn)
            .copied()
            .ok_or_else(|| DbError::Unreachable("not connected".to_owned()))?;
        Ok(self.adapter(engine))
    }
}

#[async_trait]
impl Database for Engines {
    async fn connect(
        &self,
        conn: ConnId,
        target: &ConnectionConfig,
        driver: &DriverConfig,
    ) -> Result<Posture, DbError> {
        let engine = target.engine();
        self.routes.write().await.insert(conn, engine);
        self.adapter(engine).connect(conn, target, driver).await
    }

    async fn describe(&self, conn: ConnId) -> Result<Catalog, DbError> {
        self.route(conn).await?.describe(conn).await
    }

    async fn transaction(&self, conn: ConnId) -> Result<Transaction, DbError> {
        self.route(conn).await?.transaction(conn).await
    }

    async fn execute(&self, conn: ConnId, stmt: &Statement) -> Result<Page, DbError> {
        self.route(conn).await?.execute(conn, stmt).await
    }

    async fn execute_paged(
        &self,
        conn: ConnId,
        stmt: &Statement,
        page_size: PageSize,
        cursor: Option<&PagingState>,
    ) -> Result<Page, DbError> {
        self.route(conn)
            .await?
            .execute_paged(conn, stmt, page_size, cursor)
            .await
    }
}
