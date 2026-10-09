-- Relevant projection columns from pingdotgg/t3code
-- 43f8a8de17a7ac1baa7a3cf36d681856de2d8add, migrations 005 and 055/Foundation.
CREATE TABLE projection_projects (
    project_id TEXT PRIMARY KEY, title TEXT NOT NULL, workspace_root TEXT NOT NULL,
    default_model TEXT, scripts_json TEXT NOT NULL, created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL, deleted_at TEXT
);
CREATE TABLE orchestration_v2_projection_threads (
    thread_id TEXT PRIMARY KEY, project_id TEXT NOT NULL, title TEXT NOT NULL,
    default_provider TEXT NOT NULL, provider_instance_id TEXT, runtime_mode TEXT NOT NULL,
    interaction_mode TEXT NOT NULL, active_provider_thread_id TEXT,
    created_at TEXT NOT NULL, updated_at TEXT NOT NULL, archived_at TEXT, deleted_at TEXT,
    payload_json TEXT NOT NULL
);
CREATE TABLE orchestration_v2_projection_provider_threads (
    provider_thread_id TEXT PRIMARY KEY, thread_id TEXT, owner_node_id TEXT,
    provider TEXT NOT NULL, driver TEXT, provider_instance_id TEXT,
    provider_session_id TEXT, status TEXT NOT NULL, first_run_ordinal INTEGER,
    last_run_ordinal INTEGER, updated_at TEXT NOT NULL, payload_json TEXT NOT NULL
);
CREATE TABLE orchestration_v2_projection_messages (
    message_id TEXT PRIMARY KEY, thread_id TEXT NOT NULL, run_id TEXT, node_id TEXT,
    role TEXT NOT NULL, streaming INTEGER NOT NULL, created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL, payload_json TEXT NOT NULL
);
