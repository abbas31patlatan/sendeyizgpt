CREATE TABLE settings (
    key TEXT NOT NULL,
    value_json TEXT NOT NULL,
    scope TEXT NOT NULL CHECK (scope IN ('global', 'workspace', 'profile')),
    scope_id TEXT NOT NULL DEFAULT '',
    updated_at TEXT NOT NULL,
    PRIMARY KEY (scope, scope_id, key)
);

CREATE TABLE workspaces (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    root_path TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE projects (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    root_path TEXT NOT NULL,
    git_remote TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (workspace_id, root_path)
);

CREATE TABLE agent_profiles (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    system_prompt TEXT NOT NULL,
    model_id TEXT,
    sampling_json TEXT NOT NULL,
    reasoning_json TEXT NOT NULL,
    tool_policy_json TEXT NOT NULL,
    permission_defaults_json TEXT NOT NULL,
    memory_policy_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE conversations (
    id TEXT PRIMARY KEY,
    parent_id TEXT REFERENCES conversations(id) ON DELETE SET NULL,
    workspace_id TEXT REFERENCES workspaces(id) ON DELETE SET NULL,
    agent_profile_id TEXT REFERENCES agent_profiles(id) ON DELETE SET NULL,
    title TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('active', 'archived', 'deleted')),
    pinned INTEGER NOT NULL DEFAULT 0 CHECK (pinned IN (0, 1)),
    folder TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE messages (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    parent_message_id TEXT REFERENCES messages(id) ON DELETE SET NULL,
    role TEXT NOT NULL CHECK (role IN ('system', 'developer', 'user', 'assistant', 'tool')),
    content TEXT,
    content_ref TEXT,
    reasoning_ref TEXT,
    tool_call_json TEXT,
    token_count INTEGER,
    created_at TEXT NOT NULL,
    CHECK (content IS NOT NULL OR content_ref IS NOT NULL)
);

CREATE TABLE attachments (
    id TEXT PRIMARY KEY,
    message_id TEXT REFERENCES messages(id) ON DELETE CASCADE,
    storage_path TEXT NOT NULL,
    content_hash TEXT NOT NULL UNIQUE,
    media_type TEXT NOT NULL,
    byte_size INTEGER NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE model_libraries (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    root_path TEXT NOT NULL UNIQUE,
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    last_scan_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE models (
    id TEXT PRIMARY KEY,
    library_id TEXT NOT NULL REFERENCES model_libraries(id) ON DELETE CASCADE,
    display_name TEXT NOT NULL,
    file_path TEXT NOT NULL UNIQUE,
    format TEXT NOT NULL,
    family TEXT,
    parameter_count INTEGER,
    architecture TEXT,
    quantization TEXT,
    gguf_version TEXT,
    file_size_bytes INTEGER NOT NULL,
    context_capacity INTEGER,
    vision INTEGER NOT NULL DEFAULT 0 CHECK (vision IN (0, 1)),
    tool_calling INTEGER NOT NULL DEFAULT 0 CHECK (tool_calling IN (0, 1)),
    reasoning INTEGER NOT NULL DEFAULT 0 CHECK (reasoning IN (0, 1)),
    embeddings INTEGER NOT NULL DEFAULT 0 CHECK (embeddings IN (0, 1)),
    metadata_json TEXT NOT NULL,
    metadata_hash TEXT,
    last_seen_at TEXT NOT NULL,
    UNIQUE (library_id, file_path)
);

CREATE TABLE model_profiles (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    preset TEXT NOT NULL CHECK (preset IN ('eco', 'balanced', 'performance', 'custom')),
    model_id TEXT REFERENCES models(id) ON DELETE SET NULL,
    config_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (name, model_id)
);

CREATE TABLE providers (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    base_url TEXT,
    model_catalog_json TEXT NOT NULL,
    secret_ref TEXT,
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE permission_grants (
    id TEXT PRIMARY KEY,
    scope_kind TEXT NOT NULL,
    scope_json TEXT NOT NULL,
    capabilities_json TEXT NOT NULL,
    risk_ceiling TEXT NOT NULL,
    strict_mode INTEGER NOT NULL DEFAULT 1 CHECK (strict_mode IN (0, 1)),
    expires_at TEXT,
    created_at TEXT NOT NULL,
    revoked_at TEXT
);

CREATE TABLE audit_events (
    id TEXT PRIMARY KEY,
    request_id TEXT NOT NULL,
    conversation_id TEXT,
    agent_id TEXT,
    tool_id TEXT NOT NULL,
    action_kind TEXT NOT NULL,
    risk_level TEXT NOT NULL,
    permission_decision TEXT NOT NULL,
    arguments_redacted_json TEXT,
    result_summary TEXT,
    result_ref TEXT,
    duration_ms INTEGER,
    exit_code INTEGER,
    created_at TEXT NOT NULL
);

CREATE TABLE event_sources (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    name TEXT NOT NULL,
    provider_key TEXT NOT NULL,
    config_json TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    last_update_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE automations (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    source_id TEXT REFERENCES event_sources(id) ON DELETE SET NULL,
    trigger_kind TEXT NOT NULL CHECK (trigger_kind IN ('one_shot', 'interval', 'cron', 'condition')),
    condition_json TEXT NOT NULL,
    action_json TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0, 1)),
    last_run_at TEXT,
    next_run_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE plugins (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    version TEXT NOT NULL,
    api_major INTEGER NOT NULL,
    api_minor INTEGER NOT NULL,
    kind TEXT NOT NULL,
    manifest_json TEXT NOT NULL,
    package_hash TEXT NOT NULL,
    trust_state TEXT NOT NULL CHECK (trust_state IN ('quarantined', 'enabled', 'disabled', 'rejected')),
    installed_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX idx_conversations_updated_at ON conversations(updated_at DESC);
CREATE INDEX idx_messages_conversation_created ON messages(conversation_id, created_at);
CREATE INDEX idx_models_library ON models(library_id);
CREATE INDEX idx_audit_created_at ON audit_events(created_at DESC);
CREATE INDEX idx_automations_next_run ON automations(enabled, next_run_at);
CREATE INDEX idx_events_source_update ON event_sources(enabled, last_update_at);
