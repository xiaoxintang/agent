CREATE TABLE node_metrics (
    node_id TEXT PRIMARY KEY NOT NULL REFERENCES nodes(id),
    connection_id TEXT NOT NULL,
    snapshot_json TEXT NOT NULL CHECK(json_valid(snapshot_json)),
    received_at INTEGER NOT NULL
);
