CREATE TABLE inbounds (
 id TEXT PRIMARY KEY NOT NULL, node_id TEXT NOT NULL REFERENCES nodes(id), name TEXT NOT NULL,
 protocol TEXT NOT NULL DEFAULT 'vless' CHECK(protocol='vless'), listen_address TEXT NOT NULL,
 listen_port INTEGER NOT NULL CHECK(listen_port BETWEEN 1 AND 65535), settings_json TEXT NOT NULL CHECK(json_valid(settings_json)),
 enabled INTEGER NOT NULL CHECK(enabled IN (0,1)), created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
 UNIQUE(node_id,listen_port)
);
CREATE TABLE clients (
 id TEXT PRIMARY KEY NOT NULL, display_name TEXT NOT NULL, protocol_identity_json TEXT NOT NULL CHECK(json_valid(protocol_identity_json)),
 enabled INTEGER NOT NULL CHECK(enabled IN (0,1)), expiry_at INTEGER, quota_bytes INTEGER, created_at INTEGER NOT NULL
);
CREATE TABLE client_assignments (
 id TEXT PRIMARY KEY NOT NULL, client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
 inbound_id TEXT NOT NULL REFERENCES inbounds(id) ON DELETE CASCADE, enabled INTEGER NOT NULL DEFAULT 1,
 UNIQUE(client_id,inbound_id)
);
CREATE TABLE config_revisions (
 id TEXT PRIMARY KEY NOT NULL, node_id TEXT NOT NULL REFERENCES nodes(id), revision INTEGER NOT NULL,
 desired_config_json TEXT NOT NULL CHECK(json_valid(desired_config_json)), content_hash TEXT NOT NULL,
 status TEXT NOT NULL CHECK(status IN ('pending','applied','failed')), created_at INTEGER NOT NULL, applied_at INTEGER,
 task_id TEXT REFERENCES tasks(id) ON DELETE SET NULL, UNIQUE(node_id,revision)
);
