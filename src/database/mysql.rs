use anyhow::Result;
use async_trait::async_trait;
use mysql_async::prelude::*;
use mysql_async::{Pool, OptsBuilder};

use crate::core::types::DatabaseType;
use crate::database::connection::DatabaseConnector;

pub struct MySqlConnector {
    pool: Option<Pool>,
    database_type: DatabaseType,
}

impl MySqlConnector {
    pub fn new() -> Self {
        Self {
            pool: None,
            database_type: DatabaseType::MySQL,
        }
    }
}

#[async_trait]
impl DatabaseConnector for MySqlConnector {
    async fn connect(&mut self, connection_string: &str) -> Result<()> {
        println!("Connecting to MySQL: {}", connection_string);
        
        // Parse connection string (basic parsing)
        let parts: Vec<&str> = connection_string.split('@').collect();
        if parts.len() != 2 {
            return Err(anyhow::anyhow!("Invalid MySQL connection string format"));
        }

        let user_parts: Vec<&str> = parts[0].split(':').collect();
        let host_parts: Vec<&str> = parts[1].split(':').collect();
        if user_parts.len() < 2 || host_parts.len() < 2 {
            return Err(anyhow::anyhow!("Invalid MySQL connection string format"));
        }

        let username = user_parts[0];
        let password = user_parts[1];
        let host = host_parts[0];
        let port_db = host_parts[1];
        
        let port_parts = port_db.split('/').collect::<Vec<&str>>();
        if port_parts.len() < 2 {
            return Err(anyhow::anyhow!("Invalid MySQL connection string format"));
        }
        
        let port = port_parts[0].parse::<u16>().unwrap_or(3306);
        let database = port_parts[1];

        // Create connection pool
        let pool = Pool::new(OptsBuilder::default()
            .user(Some(username))
            .pass(Some(password))
            .ip_or_hostname(host)
            .tcp_port(port)
            .db_name(Some(database)));

        // Test the connection
        let mut conn = pool.get_conn().await?;
        let _: Vec<mysql_async::Row> = conn.query("SELECT 1").await?;
        drop(conn);

        self.pool = Some(pool);
        println!("Successfully connected to MySQL database: {}", database);
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        if let Some(pool) = self.pool.take() {
            pool.disconnect().await?;
        }
        Ok(())
    }

    async fn test_connection(&self) -> Result<bool> {
        match &self.pool {
            Some(pool) => {
                match pool.get_conn().await {
                    Ok(conn) => {
                        drop(conn);
                        Ok(true)
                    }
                    Err(_) => Ok(false)
                }
            }
            None => Ok(false)
        }
    }

    fn database_type(&self) -> DatabaseType {
        self.database_type.clone()
    }
}
