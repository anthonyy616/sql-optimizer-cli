use anyhow::{anyhow, Context, Result};
use sqlparser::ast::Statement;
use sqlparser::dialect::{Dialect, MySqlDialect, PostgreSqlDialect, SQLiteDialect};
use sqlparser::parser::Parser;

use crate::core::types::DatabaseType;

fn dialect_for(db_type: DatabaseType) -> Box<dyn Dialect> {
    match db_type {
        DatabaseType::PostgreSQL => Box::new(PostgreSqlDialect {}),
        DatabaseType::MySQL => Box::new(MySqlDialect {}),
        DatabaseType::SQLite => Box::new(SQLiteDialect {}),
    }
}

pub fn ensure_read_only_select(query: &str, db_type: DatabaseType) -> Result<()> {
    let dialect = dialect_for(db_type);
    let statements = Parser::parse_sql(&*dialect, query)
        .with_context(|| format!("Failed to parse SQL query: {query}"))?;

    match statements.as_slice() {
        [Statement::Query(_)] => Ok(()),
        [_] => Err(anyhow!(
            "Row preview only supports read-only SELECT queries"
        )),
        _ => Err(anyhow!(
            "Row preview only supports a single read-only SELECT statement"
        )),
    }
}

pub fn preview_sql(query: &str, limit: usize) -> String {
    format!(
        "SELECT * FROM ({query}) AS sql_optimizer_preview LIMIT {}",
        limit
    )
}
