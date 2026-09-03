//! SQLite connection setup and transactional schema migrations.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;
use thiserror::Error;
use uuid::Uuid;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatRole {
    System,
    Developer,
    User,
    Assistant,
    Tool,
}

impl ChatRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Developer => "developer",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
        }
    }

    fn parse(value: &str) -> Result<Self, DatabaseError> {
        match value {
            "system" => Ok(Self::System),
            "developer" => Ok(Self::Developer),
            "user" => Ok(Self::User),
            "assistant" => Ok(Self::Assistant),
            "tool" => Ok(Self::Tool),
            _ => Err(DatabaseError::InvalidData(format!(
                "unknown chat role: {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationSummary {
    pub id: String,
    pub title: String,
    pub pinned: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessageRecord {
    pub id: String,
    pub conversation_id: String,
    pub role: ChatRole,
    pub content: String,
    pub created_at: String,
}

pub struct ChatStore {
    connection: Mutex<Connection>,
}

impl ChatStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DatabaseError> {
        Ok(Self {
            connection: Mutex::new(open(path)?),
        })
    }

    pub fn create_conversation(&self, title: &str) -> Result<ConversationSummary, DatabaseError> {
        let now = chrono::Utc::now().to_rfc3339();
        let conversation = ConversationSummary {
            id: Uuid::new_v4().to_string(),
            title: normalized_title(title),
            pinned: false,
            created_at: now.clone(),
            updated_at: now,
        };
        self.connection
            .lock()
            .map_err(|_| DatabaseError::LockPoisoned)?
            .execute(
                "INSERT INTO conversations(id, title, status, pinned, created_at, updated_at)
                 VALUES (?1, ?2, 'active', 0, ?3, ?4)",
                rusqlite::params![
                    conversation.id,
                    conversation.title,
                    conversation.created_at,
                    conversation.updated_at
                ],
            )
            .map_err(DatabaseError::Query)?;
        Ok(conversation)
    }

    pub fn list_conversations(&self) -> Result<Vec<ConversationSummary>, DatabaseError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| DatabaseError::LockPoisoned)?;
        let mut statement = connection
            .prepare(
                "SELECT id, title, pinned, created_at, updated_at
                 FROM conversations WHERE status = 'active'
                 ORDER BY pinned DESC, updated_at DESC LIMIT 200",
            )
            .map_err(DatabaseError::Query)?;
        statement
            .query_map([], |row| {
                Ok(ConversationSummary {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    pinned: row.get::<_, i64>(2)? != 0,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                })
            })
            .map_err(DatabaseError::Query)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(DatabaseError::Query)
    }

    pub fn list_messages(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<ChatMessageRecord>, DatabaseError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| DatabaseError::LockPoisoned)?;
        let mut statement = connection
            .prepare(
                "SELECT id, conversation_id, role, content, created_at
                 FROM messages WHERE conversation_id = ?1 ORDER BY created_at, rowid",
            )
            .map_err(DatabaseError::Query)?;
        let rows = statement
            .query_map([conversation_id], |row| {
                let role = row.get::<_, String>(2)?;
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    role,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .map_err(DatabaseError::Query)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(DatabaseError::Query)?;
        rows.into_iter()
            .map(|(id, conversation_id, role, content, created_at)| {
                Ok(ChatMessageRecord {
                    id,
                    conversation_id,
                    role: ChatRole::parse(&role)?,
                    content,
                    created_at,
                })
            })
            .collect()
    }

    pub fn append_message(
        &self,
        conversation_id: &str,
        role: ChatRole,
        content: &str,
    ) -> Result<ChatMessageRecord, DatabaseError> {
        if content.trim().is_empty() {
            return Err(DatabaseError::InvalidData(
                "message content cannot be empty".to_owned(),
            ));
        }
        let now = chrono::Utc::now().to_rfc3339();
        let message = ChatMessageRecord {
            id: Uuid::new_v4().to_string(),
            conversation_id: conversation_id.to_owned(),
            role,
            content: content.to_owned(),
            created_at: now.clone(),
        };
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| DatabaseError::LockPoisoned)?;
        let transaction = connection.transaction().map_err(DatabaseError::Query)?;
        transaction
            .execute(
                "INSERT INTO messages(id, conversation_id, role, content, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![message.id, conversation_id, role.as_str(), content, now],
            )
            .map_err(DatabaseError::Query)?;
        transaction
            .execute(
                "UPDATE conversations SET updated_at = ?2 WHERE id = ?1",
                rusqlite::params![conversation_id, message.created_at],
            )
            .map_err(DatabaseError::Query)?;
        transaction.commit().map_err(DatabaseError::Query)?;
        Ok(message)
    }
}

fn normalized_title(title: &str) -> String {
    let trimmed = title.trim();
    let source = if trimmed.is_empty() {
        "New conversation"
    } else {
        trimmed
    };
    source.chars().take(80).collect()
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
    #[error("database query failed: {0}")]
    Query(rusqlite::Error),
    #[error("database contains invalid data: {0}")]
    InvalidData(String),
    #[error("database lock is poisoned")]
    LockPoisoned,
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

    #[test]
    fn conversations_and_messages_round_trip() {
        let store = ChatStore {
            connection: Mutex::new(open_in_memory().expect("database opens")),
        };
        let conversation = store
            .create_conversation("First chat")
            .expect("conversation");
        store
            .append_message(&conversation.id, ChatRole::User, "Hello")
            .expect("message");
        assert_eq!(store.list_conversations().expect("list").len(), 1);
        let messages = store.list_messages(&conversation.id).expect("messages");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "Hello");
    }
}
