CREATE TABLE admins (
    id TEXT PRIMARY KEY NOT NULL,
    singleton INTEGER NOT NULL DEFAULT 1 UNIQUE CHECK (singleton = 1),
    username TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    disabled_at INTEGER
);

CREATE TABLE sessions (
    id TEXT PRIMARY KEY NOT NULL,
    admin_id TEXT NOT NULL REFERENCES admins(id),
    token_hash TEXT NOT NULL UNIQUE,
    expires_at INTEGER NOT NULL,
    revoked_at INTEGER,
    created_at INTEGER NOT NULL
);
CREATE INDEX sessions_expiry ON sessions(expires_at);

CREATE TABLE nodes (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'offline',
    agent_version TEXT,
    os TEXT,
    arch TEXT,
    last_seen_at INTEGER,
    created_at INTEGER NOT NULL,
    revoked_at INTEGER
);

CREATE TABLE audit_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    actor_id TEXT REFERENCES admins(id),
    node_id TEXT REFERENCES nodes(id),
    action TEXT NOT NULL,
    target TEXT NOT NULL,
    result TEXT NOT NULL CHECK (result IN ('success', 'denied')),
    created_at INTEGER NOT NULL,
    metadata_json TEXT NOT NULL CHECK (json_valid(metadata_json))
);
CREATE INDEX audit_logs_retention ON audit_logs(created_at);
