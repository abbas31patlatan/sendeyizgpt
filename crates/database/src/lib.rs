//! SQLite connection setup, transactional schema migrations and durable domain repositories.

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;
use thiserror::Error;

pub const CURRENT_SCHEMA_VERSION: i64 = 2;
const MAX_CONVERSATION_TITLE_BYTES: usize = 512;
const MAX_WORKSPACE_NAME_BYTES: usize = 256;
const MAX_WORKSPACE_PATH_BYTES: usize = 4096;
const MAX_PERSISTED_MESSAGES: usize = 2_048;
const MAX_MESSAGE_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConversationRecord {
    pub id: String,
    pub title: String,
    pub updated_at: i64,
    pub messages: Vec<MessageRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MessageRecord {
    pub id: String,
    pub role: String,
    pub content: String,
    pub reasoning: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRecord {
    pub id: String,
    pub name: String,
    pub root_path: String,
    pub created_at: i64,
    pub updated_at: i64,
}

pub struct Database {
    connection: Mutex<Connection>,
}

impl Database {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DatabaseError> {
        Ok(Self {
            connection: Mutex::new(open(path)?),
        })
    }

    pub fn open_in_memory() -> Result<Self, DatabaseError> {
        Ok(Self {
            connection: Mutex::new(open_in_memory()?),
        })
    }

    pub fn list_conversations(&self) -> Result<Vec<ConversationRecord>, DatabaseError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| DatabaseError::LockPoisoned("database connection"))?;

        let rows = {
            let mut statement = connection
                .prepare(
                    "SELECT id, title, updated_at
                     FROM conversations
                     WHERE status != 'deleted'
                     ORDER BY pinned DESC, updated_at DESC",
                )
                .map_err(DatabaseError::Repository)?;
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .map_err(DatabaseError::Repository)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(DatabaseError::Repository)?
        };

        rows.into_iter()
            .map(|(id, title, updated_at)| {
                Ok(ConversationRecord {
                    messages: read_messages(&connection, &id)?,
                    id,
                    title,
                    updated_at: parse_timestamp(&updated_at)?,
                })
            })
            .collect()
    }

    pub fn save_conversation(
        &self,
        conversation: &ConversationRecord,
    ) -> Result<(), DatabaseError> {
        validate_conversation(conversation)?;

        let mut connection = self
            .connection
            .lock()
            .map_err(|_| DatabaseError::LockPoisoned("database connection"))?;
        let transaction = connection
            .transaction()
            .map_err(DatabaseError::Repository)?;
        let timestamp = normalized_timestamp(conversation.updated_at);

        transaction
            .execute(
                "INSERT INTO conversations (
                     id, parent_id, workspace_id, agent_profile_id, title, status,
                     pinned, folder, created_at, updated_at
                 ) VALUES (?1, NULL, NULL, NULL, ?2, 'active', 0, NULL, ?3, ?3)
                 ON CONFLICT(id) DO UPDATE SET
                     title = excluded.title,
                     status = 'active',
                     updated_at = excluded.updated_at",
                params![conversation.id, conversation.title, timestamp.to_string()],
            )
            .map_err(DatabaseError::Repository)?;

        transaction
            .execute(
                "DELETE FROM messages WHERE conversation_id = ?1",
                params![conversation.id],
            )
            .map_err(DatabaseError::Repository)?;

        for message in &conversation.messages {
            transaction
                .execute(
                    "INSERT INTO messages (
                         id, conversation_id, parent_message_id, role, content,
                         content_ref, reasoning_ref, reasoning, tool_call_json,
                         token_count, created_at
                     ) VALUES (?1, ?2, NULL, ?3, ?4, NULL, NULL, ?5, NULL, NULL, ?6)",
                    params![
                        message.id,
                        conversation.id,
                        message.role,
                        message.content,
                        message.reasoning,
                        normalized_timestamp(message.created_at).to_string()
                    ],
                )
                .map_err(DatabaseError::Repository)?;
        }

        transaction.commit().map_err(DatabaseError::Repository)
    }

    pub fn delete_conversation(&self, conversation_id: &str) -> Result<bool, DatabaseError> {
        if conversation_id.trim().is_empty() {
            return Err(DatabaseError::InvalidData(
                "conversation id cannot be empty".to_owned(),
            ));
        }

        let connection = self
            .connection
            .lock()
            .map_err(|_| DatabaseError::LockPoisoned("database connection"))?;
        let deleted = connection
            .execute(
                "DELETE FROM conversations WHERE id = ?1",
                params![conversation_id],
            )
            .map_err(DatabaseError::Repository)?;
        Ok(deleted > 0)
    }

    pub fn list_workspaces(&self) -> Result<Vec<WorkspaceRecord>, DatabaseError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| DatabaseError::LockPoisoned("database connection"))?;
        let mut statement = connection
            .prepare(
                "SELECT id, name, root_path, created_at, updated_at
                 FROM workspaces
                 ORDER BY updated_at DESC",
            )
            .map_err(DatabaseError::Repository)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .map_err(DatabaseError::Repository)?;

        rows.map(|row| {
            let (id, name, root_path, created_at, updated_at) =
                row.map_err(DatabaseError::Repository)?;
            Ok(WorkspaceRecord {
                id,
                name,
                root_path,
                created_at: parse_timestamp(&created_at)?,
                updated_at: parse_timestamp(&updated_at)?,
            })
        })
        .collect()
    }

    pub fn save_workspace(&self, workspace: &WorkspaceRecord) -> Result<(), DatabaseError> {
        validate_workspace(workspace)?;

        let connection = self
            .connection
            .lock()
            .map_err(|_| DatabaseError::LockPoisoned("database connection"))?;
        let timestamp = normalized_timestamp(workspace.updated_at);
        connection
            .execute(
                "INSERT INTO workspaces (id, name, root_path, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?4)
                 ON CONFLICT(id) DO UPDATE SET
                     name = excluded.name,
                     root_path = excluded.root_path,
                     updated_at = excluded.updated_at",
                params![
                    workspace.id,
                    workspace.name,
                    workspace.root_path,
                    timestamp.to_string()
                ],
            )
            .map_err(DatabaseError::Repository)?;
        Ok(())
    }

    pub fn delete_workspace(&self, workspace_id: &str) -> Result<bool, DatabaseError> {
        if workspace_id.trim().is_empty() {
            return Err(DatabaseError::InvalidData(
                "workspace id cannot be empty".to_owned(),
            ));
        }

        let connection = self
            .connection
            .lock()
            .map_err(|_| DatabaseError::LockPoisoned("database connection"))?;
        let deleted = connection
            .execute(
                "DELETE FROM workspaces WHERE id = ?1",
                params![workspace_id],
            )
            .map_err(DatabaseError::Repository)?;
        Ok(deleted > 0)
    }
}

fn read_messages(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Vec<MessageRecord>, DatabaseError> {
    let mut statement = connection
        .prepare(
            "SELECT id, role, content, reasoning, created_at
             FROM messages
             WHERE conversation_id = ?1
             ORDER BY created_at ASC, rowid ASC",
        )
        .map_err(DatabaseError::Repository)?;
    let rows = statement
        .query_map(params![conversation_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .map_err(DatabaseError::Repository)?;

    rows.map(|row| {
        let (id, role, content, reasoning, created_at) =
            row.map_err(DatabaseError::Repository)?;
        let content = content.ok_or_else(|| {
            DatabaseError::InvalidData(format!(
                "message {id} has external content that cannot be restored yet"
            ))
        })?;
        Ok(MessageRecord {
            id,
            role,
            content,
            reasoning,
            created_at: parse_timestamp(&created_at)?,
        })
    })
    .collect()
}

fn validate_conversation(conversation: &ConversationRecord) -> Result<(), DatabaseError> {
    if conversation.id.trim().is_empty() {
        return Err(DatabaseError::InvalidData(
            "conversation id cannot be empty".to_owned(),
        ));
    }
    if conversation.id.len() > 256 {
        return Err(DatabaseError::InvalidData(
            "conversation id is too long".to_owned(),
        ));
    }
    if conversation.title.trim().is_empty()
        || conversation.title.len() > MAX_CONVERSATION_TITLE_BYTES
    {
        return Err(DatabaseError::InvalidData(
            "conversation title is empty or too long".to_owned(),
        ));
    }
    if conversation.messages.len() > MAX_PERSISTED_MESSAGES {
        return Err(DatabaseError::InvalidData(
            "conversation contains too many messages".to_owned(),
        ));
    }

    for message in &conversation.messages {
        if message.id.trim().is_empty() || message.id.len() > 256 {
            return Err(DatabaseError::InvalidData(
                "message id is empty or too long".to_owned(),
            ));
        }
        if !matches!(
            message.role.as_str(),
            "system" | "developer" | "user" | "assistant" | "tool"
        ) {
            return Err(DatabaseError::InvalidData(format!(
                "unsupported message role: {}",
                message.role
            )));
        }
        if message.content.len() > MAX_MESSAGE_BYTES {
            return Err(DatabaseError::InvalidData(
                "message content is too large".to_owned(),
            ));
        }
        if message
            .reasoning
            .as_ref()
            .is_some_and(|value| value.len() > MAX_MESSAGE_BYTES)
        {
            return Err(DatabaseError::InvalidData(
                "message reasoning is too large".to_owned(),
            ));
        }
    }

    Ok(())
}

fn validate_workspace(workspace: &WorkspaceRecord) -> Result<(), DatabaseError> {
    if workspace.id.trim().is_empty() || workspace.id.len() > 256 {
        return Err(DatabaseError::InvalidData(
            "workspace id is empty or too long".to_owned(),
        ));
    }
    if workspace.name.trim().is_empty() || workspace.name.len() > MAX_WORKSPACE_NAME_BYTES {
        return Err(DatabaseError::InvalidData(
            "workspace name is empty or too long".to_owned(),
        ));
    }
    if workspace.root_path.trim().is_empty() || workspace.root_path.len() > MAX_WORKSPACE_PATH_BYTES
    {
        return Err(DatabaseError::InvalidData(
            "workspace path is empty or too long".to_owned(),
        ));
    }
    Ok(())
}

fn normalized_timestamp(timestamp: i64) -> i64 {
    if timestamp > 0 {
        timestamp
    } else {
        Utc::now().timestamp_millis()
    }
}

fn parse_timestamp(raw: &str) -> Result<i64, DatabaseError> {
    if let Ok(timestamp) = raw.parse::<i64>() {
        return Ok(timestamp);
    }

    DateTime::parse_from_rfc3339(raw)
        .map(|value| value.timestamp_millis())
        .map_err(|error| DatabaseError::InvalidData(format!("invalid timestamp {raw}: {error}")))
}

pub fn open(path: impl AsRef<Path>) -> Result<Connection, DatabaseError> {
    let connection = Connection::open(path).map_err(DatabaseError::Open)?;
    configure(&connection)?;
    let mut connection = connection;
    migrate(&mut connection)?;
    Ok(connection)
}

pub fn open_in_memory() -> Result<Connection, DatabaseError> {
    let connection = Connection::open_in_memory().map_err(DatabaseError::Open)?;
    configure(&connection)?;
    let mut connection = connection;
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
                rusqlite::params![version, name, Utc::now().to_rfc3339()],
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
        2 => Some((
            "message_reasoning_column",
            include_str!("../migrations/0002_message_reasoning.sql"),
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
    #[error("database repository operation failed: {0}")]
    Repository(rusqlite::Error),
    #[error("database lock poisoned: {0}")]
    LockPoisoned(&'static str),
    #[error("invalid persisted data: {0}")]
    InvalidData(String),
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

    #[test]
    fn conversation_repository_round_trips_messages_and_reasoning() {
        let database = Database::open_in_memory().expect("database opens");
        let conversation = ConversationRecord {
            id: "conversation-1".to_owned(),
            title: "Repository test".to_owned(),
            updated_at: 1_700_000_000_000,
            messages: vec![
                MessageRecord {
                    id: "message-1".to_owned(),
                    role: "user".to_owned(),
                    content: "Hello".to_owned(),
                    reasoning: None,
                    created_at: 1_700_000_000_001,
                },
                MessageRecord {
                    id: "message-2".to_owned(),
                    role: "assistant".to_owned(),
                    content: "Hi".to_owned(),
                    reasoning: Some("short trace".to_owned()),
                    created_at: 1_700_000_000_002,
                },
            ],
        };

        database
            .save_conversation(&conversation)
            .expect("conversation saves");
        assert_eq!(database.list_conversations().expect("conversation loads"), vec![conversation]);
    }

    #[test]
    fn deleting_conversation_cascades_to_messages() {
        let database = Database::open_in_memory().expect("database opens");
        let conversation = ConversationRecord {
            id: "conversation-2".to_owned(),
            title: "Delete me".to_owned(),
            updated_at: 1_700_000_000_000,
            messages: Vec::new(),
        };
        database
            .save_conversation(&conversation)
            .expect("conversation saves");
        assert!(database
            .delete_conversation(&conversation.id)
            .expect("conversation deletes"));
        assert!(database.list_conversations().expect("conversation loads").is_empty());
    }
}
