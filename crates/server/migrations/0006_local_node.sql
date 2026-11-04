CREATE TABLE local_node (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    node_id TEXT NOT NULL REFERENCES nodes(id)
);
