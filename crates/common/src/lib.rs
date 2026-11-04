use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub mod configuration;
pub mod protocol;
pub mod telemetry;
pub mod xray;
pub use telemetry::{
    ByteUsage, ManagedService, MetricIssue, MetricName, MetricReason, MetricsSnapshot,
    NetworkUsage, NodeMetrics, ServiceError, ServiceQuery, ServiceReport,
};
pub use xray::{XrayError, XrayOperation, XrayRelease, XrayReport, XrayResult, XrayVersion};
pub mod storage;
pub use protocol::{
    AgentInfo, EnrollmentInput, EnrollmentResponse, TaskCommand, TaskInput, TaskOperation,
    TaskOutcome, TaskRecord,
};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Deserialize, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Clone, Deserialize, Serialize, TS)]
pub struct AdminUser {
    pub id: String,
    pub username: String,
}

#[derive(Deserialize, Serialize, TS)]
pub struct SessionResponse {
    pub admin: AdminUser,
    pub csrf_token: String,
    #[ts(type = "number")]
    pub expires_at: i64,
}

#[derive(Deserialize, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct NodeInput {
    pub name: String,
}

#[derive(Deserialize, Serialize, TS)]
pub struct Node {
    pub id: String,
    pub name: String,
    pub status: String,
    pub agent_version: Option<String>,
    pub os: Option<String>,
    pub arch: Option<String>,
    #[ts(type = "number | null")]
    pub last_seen_at: Option<i64>,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number | null")]
    pub revoked_at: Option<i64>,
}

#[derive(Deserialize, Serialize, TS)]
pub struct AuditMetadata {
    pub request_id: Option<String>,
}

#[derive(Deserialize, Serialize, TS)]
pub struct AuditEntry {
    pub id: String,
    pub actor_id: Option<String>,
    pub node_id: Option<String>,
    pub action: String,
    pub target: String,
    pub result: String,
    #[ts(type = "number")]
    pub created_at: i64,
    pub metadata: AuditMetadata,
}

#[derive(Debug, Deserialize, Serialize, TS)]
pub struct HealthResponse {
    pub status: HealthStatus,
    pub version: String,
}

#[derive(Debug, Deserialize, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    Ok,
}

#[derive(Serialize)]
pub struct ErrorResponse {
    pub error: ApiError,
}

#[derive(Serialize)]
pub struct ApiError {
    pub code: &'static str,
    pub message: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_contract_has_a_stable_wire_format() {
        let health = HealthResponse {
            status: HealthStatus::Ok,
            version: VERSION.into(),
        };
        assert_eq!(
            serde_json::to_value(health).unwrap(),
            serde_json::json!({ "status": "ok", "version": VERSION })
        );
        assert!(
            serde_json::from_str::<HealthResponse>(r#"{"status":"unknown","version":"0.1.0"}"#)
                .is_err()
        );
    }
}

#[cfg(feature = "xray-stats")]
pub mod stats;
pub mod traffic;
