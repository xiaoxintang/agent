ALTER TABLE nodes ADD COLUMN connection_id TEXT;

CREATE TABLE enrollments (
    id TEXT PRIMARY KEY NOT NULL,
    node_id TEXT NOT NULL REFERENCES nodes(id),
    token_hash TEXT NOT NULL UNIQUE,
    expires_at INTEGER NOT NULL,
    consumed_at INTEGER,
    created_at INTEGER NOT NULL
);
CREATE INDEX enrollments_node ON enrollments(node_id);

CREATE TABLE node_credentials (
    node_id TEXT PRIMARY KEY NOT NULL REFERENCES nodes(id),
    credential_hash TEXT NOT NULL UNIQUE,
    issued_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    revoked_at INTEGER
);

CREATE TABLE tasks (
    id TEXT PRIMARY KEY NOT NULL,
    node_id TEXT NOT NULL REFERENCES nodes(id),
    operation TEXT NOT NULL,
    payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
    idempotency_key TEXT NOT NULL,
    desired_revision INTEGER,
    status TEXT NOT NULL CHECK(status IN ('pending','dispatched','acked','succeeded','failed','expired','canceled')),
    attempt_count INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    deadline_at INTEGER NOT NULL,
    finished_at INTEGER,
    error_code TEXT,
    error_message TEXT,
    result_json TEXT CHECK(result_json IS NULL OR json_valid(result_json)),
    UNIQUE(node_id, idempotency_key)
);
CREATE INDEX tasks_dispatch ON tasks(node_id, status, created_at);
