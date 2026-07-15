use anyhow::Result;
use async_trait::async_trait;

use crate::core::types::{DatabaseType, QueryPlan, SchemaSnapshot};

pub fn create_connector(db_type: DatabaseType) -> Box<dyn DatabaseConnector> {
    match db_type {
        DatabaseType::PostgreSQL => Box::new(crate::database::postgresql::PostgresConnector::new()),
        DatabaseType::MySQL => Box::new(crate::database::mysql::MySqlConnector::new()),
        DatabaseType::SQLite => Box::new(crate::database::sqlite::SqliteConnector::new()),
    }
}

#[async_trait]
pub trait DatabaseConnector: Send + Sync {
    async fn connect(&mut self, connection_string: &str) -> Result<()>;
    async fn disconnect(&mut self) -> Result<()>;
    async fn test_connection(&self) -> Result<bool>;
    async fn introspect_schema(&self) -> Result<SchemaSnapshot>;
    async fn explain_query(&self, query: &str) -> Result<QueryPlan>;
    fn database_type(&self) -> crate::core::types::DatabaseType;
}
