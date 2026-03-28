use anyhow::Result;
use async_trait::async_trait;

#[async_trait]
pub trait DatabaseConnector: Send + Sync {
    async fn connect(&mut self, connection_string: &str) -> Result<()>;
    async fn disconnect(&mut self) -> Result<()>;
    async fn test_connection(&self) -> Result<bool>;
    fn database_type(&self) -> crate::core::types::DatabaseType;
}

pub fn create_connector(db_type: crate::core::types::DatabaseType) -> Box<dyn DatabaseConnector> {
    match db_type {
        crate::core::types::DatabaseType::PostgreSQL => {
            Box::new(crate::database::postgresql::PostgresConnector::new())
        }
        crate::core::types::DatabaseType::MySQL => {
            Box::new(crate::database::mysql::MySqlConnector::new())
        }
    }
}
