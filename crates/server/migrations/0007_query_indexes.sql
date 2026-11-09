CREATE INDEX client_assignments_inbound ON client_assignments(inbound_id, enabled, client_id);
CREATE INDEX traffic_node_bucket ON traffic_aggregates(node_id, bucket_start);
