ALTER TABLE clients ADD COLUMN used_bytes INTEGER NOT NULL DEFAULT 0 CHECK(used_bytes>=0);
ALTER TABLE clients ADD COLUMN policy_blocked INTEGER NOT NULL DEFAULT 0 CHECK(policy_blocked IN (0,1));
CREATE TABLE traffic_baselines (
 node_id TEXT NOT NULL REFERENCES nodes(id), client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
 instance_id TEXT NOT NULL, collected_at INTEGER NOT NULL, uplink_bytes INTEGER NOT NULL, downlink_bytes INTEGER NOT NULL,
 PRIMARY KEY(node_id,client_id)
);
CREATE TABLE traffic_aggregates (
 node_id TEXT NOT NULL REFERENCES nodes(id), client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
 bucket_start INTEGER NOT NULL, uplink_bytes INTEGER NOT NULL CHECK(uplink_bytes>=0), downlink_bytes INTEGER NOT NULL CHECK(downlink_bytes>=0),
 PRIMARY KEY(node_id,client_id,bucket_start)
);
CREATE TABLE node_traffic_state (
 node_id TEXT PRIMARY KEY NOT NULL REFERENCES nodes(id), connection_id TEXT NOT NULL, available INTEGER NOT NULL,
 received_at INTEGER NOT NULL, collected_at INTEGER
);
CREATE INDEX traffic_retention ON traffic_aggregates(bucket_start);
CREATE TABLE traffic_totals (
 node_id TEXT NOT NULL REFERENCES nodes(id), client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
 uplink_bytes INTEGER NOT NULL, downlink_bytes INTEGER NOT NULL,
 PRIMARY KEY(node_id,client_id)
);
