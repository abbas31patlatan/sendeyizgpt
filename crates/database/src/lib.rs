//! SQLite connection setup, transactional schema migrations and durable domain repositories.

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;
use thiserror::Error;

pub const CURRENT_SCHEMA_VERSION: i64 = 3;
const MAX_CONVERSATION_TITLE_BYTES: usize = 512;
const MAX_WORKSPACE_NAME_BYTES: usize = 256;
const MAX_WORKSPACE_PATH_BYTES: usize = 4096;
const MAX_PERSISTED_MESSAGES: usize = 2_048;
const MAX_MESSAGE_BYTES: usize = 256 * 1024;
const MAX_MODEL_METADATA_BYTES: usize = 4 * 1024 * 1024;
const MAX_MODEL_PROFILE_BYTES: usize = 256 * 1024;
const MAX_MODEL_NAME_BYTES: usize = 512;
const MAX_AUTOMATION_NAME_BYTES: usize = 256;

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelLibraryRecord {
    pub id: String,
    pub name: String,
    pub root_path: String,
    pub enabled: bool,
    pub last_scan_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelRecord {
    pub id: String,
    pub library_id: String,
    pub display_name: String,
    pub file_path: String,
    pub format: String,
    pub family: Option<String>,
    pub parameter_count: Option<u64>,
    pub architecture: Option<String>,
    pub quantization: Option<String>,
    pub gguf_version: Option<String>,
    pub file_size_bytes: u64,
    pub context_capacity: Option<u32>,
    pub vision: bool,
    pub tool_calling: bool,
    pub reasoning: bool,
    pub embeddings: bool,
    pub metadata_hash: Option<String>,
    pub last_seen_at: i64,
    #[serde(skip)]
    pub metadata_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelProfileRecord {
    pub id: String,
    pub name: String,
    pub preset: String,
    pub model_id: Option<String>,
    pub config_json: String,
    pub created_at: i64,
    pub updated_at: i64,
}

pub struct AutomationRecord {
    pub id: String,
    pub name: String,
    pub prompt: String,
    pub interval_minutes: u32,
    pub enabled: bool,
    pub last_run_at: Option<i64>,
    pub next_run_at: Option<i64>,
    pub last_status: String,
    pub last_error: Option<String>,
    pub last_conversation_id: Option<String>,
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

    pub fn list_model_libraries(&self) -> Result<Vec<ModelLibraryRecord>, DatabaseError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| DatabaseError::LockPoisoned("database connection"))?;
        let mut statement = connection
            .prepare(
                "SELECT id, name, root_path, enabled, last_scan_at, created_at, updated_at
                 FROM model_libraries
                 ORDER BY name COLLATE NOCASE ASC, id ASC",
            )
            .map_err(DatabaseError::Repository)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            })
            .map_err(DatabaseError::Repository)?;

        rows.map(|row| {
            let (id, name, root_path, enabled, last_scan_at, created_at, updated_at) =
                row.map_err(DatabaseError::Repository)?;
            Ok(ModelLibraryRecord {
                id,
                name,
                root_path,
                enabled: bool_from_sql(enabled, "model library enabled")?,
                last_scan_at: optional_timestamp(last_scan_at)?,
                created_at: parse_timestamp(&created_at)?,
                updated_at: parse_timestamp(&updated_at)?,
            })
        })
        .collect()
    }

    pub fn get_model_library(
        &self,
        library_id: &str,
    ) -> Result<Option<ModelLibraryRecord>, DatabaseError> {
        validate_id(library_id, "model library id")?;
        let connection = self
            .connection
            .lock()
            .map_err(|_| DatabaseError::LockPoisoned("database connection"))?;
        let row = connection
            .query_row(
                "SELECT id, name, root_path, enabled, last_scan_at, created_at, updated_at
                 FROM model_libraries
                 WHERE id = ?1",
                params![library_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                },
            )
            .optional()
            .map_err(DatabaseError::Repository)?;

        row.map(
            |(id, name, root_path, enabled, last_scan_at, created_at, updated_at)| {
                Ok(ModelLibraryRecord {
                    id,
                    name,
                    root_path,
                    enabled: bool_from_sql(enabled, "model library enabled")?,
                    last_scan_at: optional_timestamp(last_scan_at)?,
                    created_at: parse_timestamp(&created_at)?,
                    updated_at: parse_timestamp(&updated_at)?,
                })
            },
        )
        .transpose()
    }

    pub fn save_model_library(&self, library: &ModelLibraryRecord) -> Result<(), DatabaseError> {
        validate_model_library(library)?;
        let connection = self
            .connection
            .lock()
            .map_err(|_| DatabaseError::LockPoisoned("database connection"))?;
        let timestamp = normalized_timestamp(library.updated_at);
        let last_scan_at = library
            .last_scan_at
            .map(normalized_timestamp)
            .map(|value| value.to_string());
        connection
            .execute(
                "INSERT INTO model_libraries (
                     id, name, root_path, enabled, last_scan_at, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
                 ON CONFLICT(id) DO UPDATE SET
                     name = excluded.name,
                     root_path = excluded.root_path,
                     enabled = excluded.enabled,
                     last_scan_at = excluded.last_scan_at,
                     updated_at = excluded.updated_at",
                params![
                    library.id,
                    library.name,
                    library.root_path,
                    i64::from(library.enabled),
                    last_scan_at,
                    timestamp.to_string()
                ],
            )
            .map_err(DatabaseError::Repository)?;
        Ok(())
    }

    pub fn delete_model_library(&self, library_id: &str) -> Result<bool, DatabaseError> {
        validate_id(library_id, "model library id")?;
        let connection = self
            .connection
            .lock()
            .map_err(|_| DatabaseError::LockPoisoned("database connection"))?;
        let deleted = connection
            .execute(
                "DELETE FROM model_libraries WHERE id = ?1",
                params![library_id],
            )
            .map_err(DatabaseError::Repository)?;
        Ok(deleted > 0)
    }

    pub fn list_models(&self, library_id: Option<&str>) -> Result<Vec<ModelRecord>, DatabaseError> {
        if let Some(library_id) = library_id {
            validate_id(library_id, "model library id")?;
        }
        let connection = self
            .connection
            .lock()
            .map_err(|_| DatabaseError::LockPoisoned("database connection"))?;
        let mut statement = connection
            .prepare(
                "SELECT id, library_id, display_name, file_path, format, family,
                        parameter_count, architecture, quantization, gguf_version,
                        file_size_bytes, context_capacity, vision, tool_calling,
                        reasoning, embeddings, metadata_hash, metadata_json, last_seen_at
                 FROM models
                 WHERE (?1 IS NULL OR library_id = ?1)
                 ORDER BY display_name COLLATE NOCASE ASC, file_path ASC",
            )
            .map_err(DatabaseError::Repository)?;
        let rows = statement
            .query_map(params![library_id], model_row)
            .map_err(DatabaseError::Repository)?;

        rows.map(|row| {
            let row = row.map_err(DatabaseError::Repository)?;
            model_from_row(row)
        })
        .collect()
    }

    pub fn get_model(&self, model_id: &str) -> Result<Option<ModelRecord>, DatabaseError> {
        validate_id(model_id, "model id")?;
        let connection = self
            .connection
            .lock()
            .map_err(|_| DatabaseError::LockPoisoned("database connection"))?;
        connection
            .query_row(
                "SELECT id, library_id, display_name, file_path, format, family,
                        parameter_count, architecture, quantization, gguf_version,
                        file_size_bytes, context_capacity, vision, tool_calling,
                        reasoning, embeddings, metadata_hash, metadata_json, last_seen_at
                 FROM models
                 WHERE id = ?1",
                params![model_id],
                model_row,
            )
            .optional()
            .map_err(DatabaseError::Repository)?
            .map(model_from_row)
            .transpose()
    }

    pub fn replace_model_library_models(
        &self,
        library_id: &str,
        models: &[ModelRecord],
        scan_timestamp: i64,
    ) -> Result<(), DatabaseError> {
        validate_id(library_id, "model library id")?;
        let timestamp = normalized_timestamp(scan_timestamp);
        for model in models {
            validate_model(model)?;
            if model.library_id != library_id {
                return Err(DatabaseError::InvalidData(
                    "scanned model belongs to another library".to_owned(),
                ));
            }
        }

        let mut connection = self
            .connection
            .lock()
            .map_err(|_| DatabaseError::LockPoisoned("database connection"))?;
        let transaction = connection
            .transaction()
            .map_err(DatabaseError::Repository)?;
        let library_exists: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM model_libraries WHERE id = ?1",
                params![library_id],
                |row| row.get(0),
            )
            .map_err(DatabaseError::Repository)?;
        if library_exists == 0 {
            return Err(DatabaseError::InvalidData(
                "model library does not exist".to_owned(),
            ));
        }

        let timestamp_text = timestamp.to_string();
        for model in models {
            transaction
                .execute(
                    "INSERT INTO models (
                         id, library_id, display_name, file_path, format, family,
                         parameter_count, architecture, quantization, gguf_version,
                         file_size_bytes, context_capacity, vision, tool_calling,
                         reasoning, embeddings, metadata_json, metadata_hash, last_seen_at
                     ) VALUES (
                         ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                         ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19
                     )
                     ON CONFLICT(id) DO UPDATE SET
                         library_id = excluded.library_id,
                         display_name = excluded.display_name,
                         file_path = excluded.file_path,
                         format = excluded.format,
                         family = excluded.family,
                         parameter_count = excluded.parameter_count,
                         architecture = excluded.architecture,
                         quantization = excluded.quantization,
                         gguf_version = excluded.gguf_version,
                         file_size_bytes = excluded.file_size_bytes,
                         context_capacity = excluded.context_capacity,
                         vision = excluded.vision,
                         tool_calling = excluded.tool_calling,
                         reasoning = excluded.reasoning,
                         embeddings = excluded.embeddings,
                         metadata_json = excluded.metadata_json,
                         metadata_hash = excluded.metadata_hash,
                         last_seen_at = excluded.last_seen_at",
                    params![
                        model.id,
                        model.library_id,
                        model.display_name,
                        model.file_path,
                        model.format,
                        model.family,
                        sql_i64(model.parameter_count, "parameter count")?,
                        model.architecture,
                        model.quantization,
                        model.gguf_version,
                        sql_i64(Some(model.file_size_bytes), "file size")?,
                        sql_i64(model.context_capacity.map(u64::from), "context capacity")?,
                        i64::from(model.vision),
                        i64::from(model.tool_calling),
                        i64::from(model.reasoning),
                        i64::from(model.embeddings),
                        model.metadata_json,
                        model.metadata_hash,
                        timestamp_text,
                    ],
                )
                .map_err(DatabaseError::Repository)?;
        }

        transaction
            .execute(
                "DELETE FROM models
                 WHERE library_id = ?1 AND last_seen_at != ?2",
                params![library_id, timestamp.to_string()],
            )
            .map_err(DatabaseError::Repository)?;
        transaction
            .execute(
                "UPDATE model_libraries
                 SET last_scan_at = ?1, updated_at = ?1
                 WHERE id = ?2",
                params![timestamp.to_string(), library_id],
            )
            .map_err(DatabaseError::Repository)?;
        transaction.commit().map_err(DatabaseError::Repository)
    }

    pub fn list_model_profiles(&self) -> Result<Vec<ModelProfileRecord>, DatabaseError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| DatabaseError::LockPoisoned("database connection"))?;
        let mut statement = connection
            .prepare(
                "SELECT id, name, preset, model_id, config_json, created_at, updated_at
                 FROM model_profiles
                 ORDER BY updated_at DESC, name COLLATE NOCASE ASC",
            )
            .map_err(DatabaseError::Repository)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            })
            .map_err(DatabaseError::Repository)?;

        rows.map(|row| {
            let (id, name, preset, model_id, config_json, created_at, updated_at) =
                row.map_err(DatabaseError::Repository)?;
            Ok(ModelProfileRecord {
                id,
                name,
                preset,
                model_id,
                config_json,
                created_at: parse_timestamp(&created_at)?,
                updated_at: parse_timestamp(&updated_at)?,
            })
        })
        .collect()
    }

    pub fn save_model_profile(&self, profile: &ModelProfileRecord) -> Result<(), DatabaseError> {
        validate_model_profile(profile)?;
        let connection = self
            .connection
            .lock()
            .map_err(|_| DatabaseError::LockPoisoned("database connection"))?;
        let timestamp = normalized_timestamp(profile.updated_at);
        connection
            .execute(
                "INSERT INTO model_profiles (
                     id, name, preset, model_id, config_json, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
                 ON CONFLICT(id) DO UPDATE SET
                     name = excluded.name,
                     preset = excluded.preset,
                     model_id = excluded.model_id,
                     config_json = excluded.config_json,
                     updated_at = excluded.updated_at",
                params![
                    profile.id,
                    profile.name,
                    profile.preset,
                    profile.model_id,
                    profile.config_json,
                    timestamp.to_string()
                ],
            )
            .map_err(DatabaseError::Repository)?;
        Ok(())
    }

    pub fn delete_model_profile(&self, profile_id: &str) -> Result<bool, DatabaseError> {
        validate_id(profile_id, "model profile id")?;
        let connection = self
            .connection
            .lock()
            .map_err(|_| DatabaseError::LockPoisoned("database connection"))?;
        let deleted = connection
            .execute(
                "DELETE FROM model_profiles WHERE id = ?1",
                params![profile_id],
            )
            .map_err(DatabaseError::Repository)?;
        Ok(deleted > 0)
    }
    pub fn list_automations(&self) -> Result<Vec<AutomationRecord>, DatabaseError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| DatabaseError::LockPoisoned("database connection"))?;
        let mut statement = connection
            .prepare(
                "SELECT id, name, prompt, interval_minutes, enabled,
                        last_run_at, next_run_at, last_status, last_error,
                        last_conversation_id, created_at, updated_at
                 FROM automations
                 WHERE trigger_kind = 'interval'
                 ORDER BY enabled DESC, updated_at DESC, name COLLATE NOCASE ASC",
            )
            .map_err(DatabaseError::Repository)?;
        let rows = statement
            .query_map([], automation_row)
            .map_err(DatabaseError::Repository)?;

        rows.map(|row| {
            let row = row.map_err(DatabaseError::Repository)?;
            automation_from_row(row)
        })
        .collect()
    }

    pub fn save_automation(&self, automation: &AutomationRecord) -> Result<(), DatabaseError> {
        validate_automation(automation)?;
        let connection = self
            .connection
            .lock()
            .map_err(|_| DatabaseError::LockPoisoned("database connection"))?;
        let created_at = normalized_timestamp(automation.created_at).to_string();
        let updated_at = normalized_timestamp(automation.updated_at).to_string();
        let last_run_at = automation
            .last_run_at
            .map(normalized_timestamp)
            .map(|value| value.to_string());
        let next_run_at = automation
            .next_run_at
            .map(normalized_timestamp)
            .map(|value| value.to_string());

        connection
            .execute(
                "INSERT INTO automations (
                     id, name, source_id, trigger_kind, condition_json, action_json,
                     enabled, last_run_at, next_run_at, created_at, updated_at,
                     prompt, interval_minutes, last_status, last_error, last_conversation_id
                 ) VALUES (
                     ?1, ?2, NULL, 'interval', ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                     ?10, ?11, ?12, ?13, ?14
                 )
                 ON CONFLICT(id) DO UPDATE SET
                     name = excluded.name,
                     trigger_kind = 'interval',
                     condition_json = excluded.condition_json,
                     action_json = excluded.action_json,
                     enabled = excluded.enabled,
                     last_run_at = excluded.last_run_at,
                     next_run_at = excluded.next_run_at,
                     updated_at = excluded.updated_at,
                     prompt = excluded.prompt,
                     interval_minutes = excluded.interval_minutes,
                     last_status = excluded.last_status,
                     last_error = excluded.last_error,
                     last_conversation_id = excluded.last_conversation_id",
                params![
                    automation.id,
                    automation.name,
                    r#"{"kind":"interval"}"#,
                    r#"{"kind":"chat"}"#,
                    i64::from(automation.enabled),
                    last_run_at,
                    next_run_at,
                    created_at,
                    updated_at,
                    automation.prompt,
                    i64::from(automation.interval_minutes),
                    automation.last_status,
                    automation.last_error,
                    automation.last_conversation_id,
                ],
            )
            .map_err(DatabaseError::Repository)?;
        Ok(())
    }

    pub fn delete_automation(&self, automation_id: &str) -> Result<bool, DatabaseError> {
        validate_id(automation_id, "automation id")?;
        let connection = self
            .connection
            .lock()
            .map_err(|_| DatabaseError::LockPoisoned("database connection"))?;
        let deleted = connection
            .execute(
                "DELETE FROM automations WHERE id = ?1",
                params![automation_id],
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
        let (id, role, content, reasoning, created_at) = row.map_err(DatabaseError::Repository)?;
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

type AutomationSqlRow = (
    String,
    String,
    String,
    i64,
    i64,
    Option<String>,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    String,
    String,
);

fn automation_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AutomationSqlRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
        row.get(11)?,
    ))
}

fn automation_from_row(row: AutomationSqlRow) -> Result<AutomationRecord, DatabaseError> {
    let (
        id,
        name,
        prompt,
        interval_minutes,
        enabled,
        last_run_at,
        next_run_at,
        last_status,
        last_error,
        last_conversation_id,
        created_at,
        updated_at,
    ) = row;

    let automation = AutomationRecord {
        id,
        name,
        prompt,
        interval_minutes: u32::try_from(interval_minutes).map_err(|_| {
            DatabaseError::InvalidData("automation interval is outside the supported range".to_owned())
        })?,
        enabled: bool_from_sql(enabled, "automation enabled")?,
        last_run_at: optional_timestamp(last_run_at)?,
        next_run_at: optional_timestamp(next_run_at)?,
        last_status,
        last_error,
        last_conversation_id,
        created_at: parse_timestamp(&created_at)?,
        updated_at: parse_timestamp(&updated_at)?,
    };
    validate_automation(&automation)?;
    Ok(automation)
}

type ModelSqlRow = (
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<i64>,
    Option<String>,
    Option<String>,
    Option<String>,
    i64,
    Option<i64>,
    i64,
    i64,
    i64,
    i64,
    Option<String>,
    String,
    String,
);

fn model_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ModelSqlRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
        row.get(11)?,
        row.get(12)?,
        row.get(13)?,
        row.get(14)?,
        row.get(15)?,
        row.get(16)?,
        row.get(17)?,
        row.get(18)?,
    ))
}

fn model_from_row(row: ModelSqlRow) -> Result<ModelRecord, DatabaseError> {
    let (
        id,
        library_id,
        display_name,
        file_path,
        format,
        family,
        parameter_count,
        architecture,
        quantization,
        gguf_version,
        file_size_bytes,
        context_capacity,
        vision,
        tool_calling,
        reasoning,
        embeddings,
        metadata_hash,
        metadata_json,
        last_seen_at,
    ) = row;

    Ok(ModelRecord {
        id,
        library_id,
        display_name,
        file_path,
        format,
        family,
        parameter_count: optional_u64(parameter_count, "model parameter count")?,
        architecture,
        quantization,
        gguf_version,
        file_size_bytes: required_u64(file_size_bytes, "model file size")?,
        context_capacity: optional_u32(context_capacity, "model context capacity")?,
        vision: bool_from_sql(vision, "model vision capability")?,
        tool_calling: bool_from_sql(tool_calling, "model tool capability")?,
        reasoning: bool_from_sql(reasoning, "model reasoning capability")?,
        embeddings: bool_from_sql(embeddings, "model embeddings capability")?,
        metadata_hash,
        last_seen_at: parse_timestamp(&last_seen_at)?,
        metadata_json,
    })
}

fn validate_id(value: &str, label: &str) -> Result<(), DatabaseError> {
    if value.trim().is_empty() || value.len() > 256 {
        return Err(DatabaseError::InvalidData(format!(
            "{label} is empty or too long"
        )));
    }
    Ok(())
}

fn validate_model_library(library: &ModelLibraryRecord) -> Result<(), DatabaseError> {
    validate_id(&library.id, "model library id")?;
    if library.name.trim().is_empty() || library.name.len() > MAX_MODEL_NAME_BYTES {
        return Err(DatabaseError::InvalidData(
            "model library name is empty or too long".to_owned(),
        ));
    }
    if library.root_path.trim().is_empty() || library.root_path.len() > MAX_WORKSPACE_PATH_BYTES {
        return Err(DatabaseError::InvalidData(
            "model library path is empty or too long".to_owned(),
        ));
    }
    Ok(())
}

fn validate_model(model: &ModelRecord) -> Result<(), DatabaseError> {
    validate_id(&model.id, "model id")?;
    validate_id(&model.library_id, "model library id")?;
    if model.display_name.trim().is_empty() || model.display_name.len() > MAX_MODEL_NAME_BYTES {
        return Err(DatabaseError::InvalidData(
            "model display name is empty or too long".to_owned(),
        ));
    }
    if model.file_path.trim().is_empty() || model.file_path.len() > MAX_WORKSPACE_PATH_BYTES {
        return Err(DatabaseError::InvalidData(
            "model file path is empty or too long".to_owned(),
        ));
    }
    if !matches!(model.format.as_str(), "gguf" | "safetensors" | "unknown") {
        return Err(DatabaseError::InvalidData(format!(
            "unsupported model format: {}",
            model.format
        )));
    }
    if model.metadata_json.len() > MAX_MODEL_METADATA_BYTES {
        return Err(DatabaseError::InvalidData(
            "model metadata is too large".to_owned(),
        ));
    }
    serde_json::from_str::<serde_json::Value>(&model.metadata_json).map_err(|error| {
        DatabaseError::InvalidData(format!("model metadata is not valid JSON: {error}"))
    })?;
    if model
        .metadata_hash
        .as_ref()
        .is_some_and(|value| value.len() > 128)
    {
        return Err(DatabaseError::InvalidData(
            "model metadata hash is too long".to_owned(),
        ));
    }
    Ok(())
}

fn validate_automation(automation: &AutomationRecord) -> Result<(), DatabaseError> {
    validate_id(&automation.id, "automation id")?;
    if automation.name.trim().is_empty() || automation.name.len() > MAX_AUTOMATION_NAME_BYTES {
        return Err(DatabaseError::InvalidData(
            "automation name is empty or too long".to_owned(),
        ));
    }
    if automation.prompt.trim().is_empty() || automation.prompt.len() > MAX_MESSAGE_BYTES {
        return Err(DatabaseError::InvalidData(
            "automation prompt is empty or too large".to_owned(),
        ));
    }
    if !(1..=10_080).contains(&automation.interval_minutes) {
        return Err(DatabaseError::InvalidData(
            "automation interval must be between 1 and 10080 minutes".to_owned(),
        ));
    }
    if !matches!(
        automation.last_status.as_str(),
        "idle" | "running" | "success" | "error" | "cancelled"
    ) {
        return Err(DatabaseError::InvalidData(format!(
            "unsupported automation status: {}",
            automation.last_status
        )));
    }
    if automation
        .last_error
        .as_ref()
        .is_some_and(|value| value.len() > MAX_MESSAGE_BYTES)
    {
        return Err(DatabaseError::InvalidData(
            "automation error is too large".to_owned(),
        ));
    }
    if let Some(conversation_id) = &automation.last_conversation_id {
        validate_id(conversation_id, "automation conversation id")?;
    }
    Ok(())
}

fn validate_model_profile(profile: &ModelProfileRecord) -> Result<(), DatabaseError> {
    validate_id(&profile.id, "model profile id")?;
    if profile.name.trim().is_empty() || profile.name.len() > MAX_MODEL_NAME_BYTES {
        return Err(DatabaseError::InvalidData(
            "model profile name is empty or too long".to_owned(),
        ));
    }
    if !matches!(
        profile.preset.as_str(),
        "eco" | "balanced" | "performance" | "custom"
    ) {
        return Err(DatabaseError::InvalidData(format!(
            "unsupported model profile preset: {}",
            profile.preset
        )));
    }
    if profile.config_json.len() > MAX_MODEL_PROFILE_BYTES {
        return Err(DatabaseError::InvalidData(
            "model profile configuration is too large".to_owned(),
        ));
    }
    serde_json::from_str::<serde_json::Value>(&profile.config_json).map_err(|error| {
        DatabaseError::InvalidData(format!(
            "model profile configuration is not valid JSON: {error}"
        ))
    })?;
    if let Some(model_id) = &profile.model_id {
        validate_id(model_id, "model profile model id")?;
    }
    Ok(())
}

fn optional_u64(value: Option<i64>, label: &str) -> Result<Option<u64>, DatabaseError> {
    value
        .map(|value| {
            u64::try_from(value)
                .map_err(|_| DatabaseError::InvalidData(format!("{label} cannot be negative")))
        })
        .transpose()
}

fn required_u64(value: i64, label: &str) -> Result<u64, DatabaseError> {
    u64::try_from(value)
        .map_err(|_| DatabaseError::InvalidData(format!("{label} cannot be negative")))
}

fn optional_u32(value: Option<i64>, label: &str) -> Result<Option<u32>, DatabaseError> {
    value
        .map(|value| {
            u32::try_from(value).map_err(|_| {
                DatabaseError::InvalidData(format!("{label} is outside the supported range"))
            })
        })
        .transpose()
}

fn sql_i64(value: Option<u64>, label: &str) -> Result<Option<i64>, DatabaseError> {
    value
        .map(|value| {
            i64::try_from(value).map_err(|_| {
                DatabaseError::InvalidData(format!("{label} exceeds SQLite integer capacity"))
            })
        })
        .transpose()
}

fn bool_from_sql(value: i64, label: &str) -> Result<bool, DatabaseError> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(DatabaseError::InvalidData(format!(
            "{label} must be 0 or 1"
        ))),
    }
}

fn optional_timestamp(value: Option<String>) -> Result<Option<i64>, DatabaseError> {
    value.map(|value| parse_timestamp(&value)).transpose()
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
        3 => Some((
            "automation_execution_state",
            include_str!("../migrations/0003_automation_execution_state.sql"),
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
        assert_eq!(
            database.list_conversations().expect("conversation loads"),
            vec![conversation]
        );
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
        assert!(
            database
                .delete_conversation(&conversation.id)
                .expect("conversation deletes")
        );
        assert!(
            database
                .list_conversations()
                .expect("conversation loads")
                .is_empty()
        );
    }

    #[test]
    fn model_library_repository_round_trips_and_replaces_snapshot() {
        let database = Database::open_in_memory().expect("database opens");
        let library = ModelLibraryRecord {
            id: "library-1".to_owned(),
            name: "Local models".to_owned(),
            root_path: "/models".to_owned(),
            enabled: true,
            last_scan_at: None,
            created_at: 1_700_000_000_000,
            updated_at: 1_700_000_000_000,
        };
        database
            .save_model_library(&library)
            .expect("library saves");

        let model = ModelRecord {
            id: "gguf-model-1".to_owned(),
            library_id: library.id.clone(),
            display_name: "Test model".to_owned(),
            file_path: "/models/test.gguf".to_owned(),
            format: "gguf".to_owned(),
            family: Some("llama".to_owned()),
            parameter_count: Some(7_000_000_000),
            architecture: Some("llama".to_owned()),
            quantization: Some("Q4_K_M".to_owned()),
            gguf_version: Some("3".to_owned()),
            file_size_bytes: 4_500_000_000,
            context_capacity: Some(8_192),
            vision: false,
            tool_calling: false,
            reasoning: false,
            embeddings: false,
            metadata_hash: Some("hash".to_owned()),
            last_seen_at: 1_700_000_000_100,
            metadata_json: "{}".to_owned(),
        };
        database
            .replace_model_library_models(
                &library.id,
                std::slice::from_ref(&model),
                1_700_000_000_100,
            )
            .expect("model snapshot saves");

        let libraries = database.list_model_libraries().expect("libraries load");
        assert_eq!(libraries.len(), 1);
        assert_eq!(libraries[0].last_scan_at, Some(1_700_000_000_100));
        let models = database
            .list_models(Some(&library.id))
            .expect("models load");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].display_name, model.display_name);
        assert_eq!(models[0].metadata_json, "{}");

        database
            .replace_model_library_models(&library.id, &[], 1_700_000_000_200)
            .expect("empty snapshot saves");
        assert!(
            database
                .list_models(None)
                .expect("models reload")
                .is_empty()
        );
    }
    #[test]
    fn automation_repository_round_trips_and_deletes() {
        let database = Database::open_in_memory().expect("database opens");
        let automation = AutomationRecord {
            id: "automation-1".to_owned(),
            name: "Project brief".to_owned(),
            prompt: "Summarize the project status.".to_owned(),
            interval_minutes: 60,
            enabled: true,
            last_run_at: Some(1_700_000_000_000),
            next_run_at: Some(1_700_000_003_600),
            last_status: "success".to_owned(),
            last_error: None,
            last_conversation_id: Some("conversation-1".to_owned()),
            created_at: 1_700_000_000_000,
            updated_at: 1_700_000_000_100,
        };

        database
            .save_automation(&automation)
            .expect("automation saves");
        assert_eq!(
            database.list_automations().expect("automations load"),
            vec![automation]
        );
        assert!(database
            .delete_automation("automation-1")
            .expect("automation deletes"));
        assert!(database
            .list_automations()
            .expect("automations reload")
            .is_empty());
    }

}
