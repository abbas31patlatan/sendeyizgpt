//! SQLite connection setup and transactional schema migrations.

use rusqlite::Connection;
use std::path::Path;
use thiserror::Error;

pub const CURRENT_SCHEMA_VERSION: i64 = 1;

pub fn open(path: impl AsRef<Path>) -> Result<Connection, DatabaseError> {
    let mut connection = Connection::open(path).map_err(DatabaseError::Open)?;
    configure(&connection)?;
    migrate(&mut connection)?;
    Ok(connection)
}

pub fn open_in_memory() -> Result<Connection, DatabaseError> {
    let mut connection = Connection::open_in_memory().map_err(DatabaseError::Open)?;
    configure(&connection)?;
    migrate(&mut connection)?;
    Ok(connection)
}

fn configure(connection: &Connection) -> Result<(), DatabaseError> {
    connection
        .execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;
             PRAGMA synchronous = NORMAL;
             PRAGMA journal_mode = WAL;",
        )
        .map_err(DatabaseError::Configure)
}

pub fn migrate(connection: &mut Connection) -> Result<MigrationReport, DatabaseError> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                 version INTEGER PRIMARY KEY,
                 name TEXT NOT NULL,
                 applied_at TEXT NOT NULL
             );",
        )
        .map_err(DatabaseError::Migration)?;

    let current: i64 = connection
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )
        .map_err(DatabaseError::Migration)?;

    if current > CURRENT_SCHEMA_VERSION {
        return Err(DatabaseError::FutureSchema {
            current,
            supported: CURRENT_SCHEMA_VERSION,
        });
    }

    let mut applied_versions = Vec::new();
    for version in (current + 1)..=CURRENT_SCHEMA_VERSION {
        let (name, sql) = migration(version).ok_or(DatabaseError::UnknownMigration(version))?;
        let transaction = connection.transaction().map_err(DatabaseError::Migration)?;
        transaction
            .execute_batch(sql)
            .map_err(DatabaseError::Migration)?;
        transaction
            .execute(
                "INSERT INTO schema_migrations(version, name, applied_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![version, name, chrono::Utc::now().to_rfc3339()],
            )
            .map_err(DatabaseError::Migration)?;
        transaction.commit().map_err(DatabaseError::Migration)?;
        applied_versions.push(version);
    }

    Ok(MigrationReport {
        current_version: CURRENT_SCHEMA_VERSION,
        applied_versions,
    })
}

fn migration(version: i64) -> Option<(&'static str, &'static str)> {
    match version {
        1 => Some((
            "initial_domain_schema",
            include_str!("../migrations/0001_initial.sql"),
        )),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationReport {
    pub current_version: i64,
    pub applied_versions: Vec<i64>,
}

#[derive(Debug, Error)]
pub enum DatabaseError {
    #[error("database open failed: {0}")]
    Open(rusqlite::Error),
    #[error("database configuration failed: {0}")]
    Configure(rusqlite::Error),
    #[error("database migration failed: {0}")]
    Migration(rusqlite::Error),
    #[error("database schema {current} is newer than supported schema {supported}")]
    FutureSchema { current: i64, supported: i64 },
    #[error("database migration {0} is not defined")]
    UnknownMigration(i64),
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::OptionalExtension;

    #[test]
    fn creates_schema_and_enables_foreign_keys() {
        let connection = open_in_memory().expect("database opens");
        let foreign_keys: i64 = connection
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .expect("pragma reads");
        assert_eq!(foreign_keys, 1);
        let version: i64 = connection
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .expect("migration reads");
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn migration_is_idempotent() {
        let mut connection = open_in_memory().expect("database opens");
        let report = migrate(&mut connection).expect("migration repeats");
        assert!(report.applied_versions.is_empty());
        let count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'audit_events'",
                [],
                |row| row.get(0),
            )
            .expect("table exists");
        assert_eq!(count, 1);
        let _ = connection
            .query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
            .optional()
            .expect("query works");
    }
}
