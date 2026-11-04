use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339};
use ts_rs::TS;
use uuid::Uuid;

pub const PROTOCOL_VERSION: u16 = 6;
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;
pub const HEARTBEAT_SECONDS: u64 = 15;
pub const OFFLINE_SECONDS: u64 = 45;

#[derive(Deserialize, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct EnrollmentInput {
    pub node_id: String,
}

#[derive(Deserialize, Serialize, TS)]
pub struct EnrollmentResponse {
    pub node_id: String,
    pub token: String,
    #[ts(type = "number")]
    pub expires_at: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, TS)]
#[serde(rename_all = "snake_case")]
pub enum TaskOperation {
    AgentStatus,
    ServiceStatus(crate::ServiceQuery),
    Xray(crate::XrayOperation),
    ApplyConfig(crate::configuration::ConfigRef),
}

#[derive(Deserialize, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct TaskInput {
    pub operation: TaskOperation,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, TS)]
#[serde(deny_unknown_fields)]
pub struct TaskCommand {
    pub id: String,
    pub operation: TaskOperation,
    #[ts(type = "number")]
    pub deadline_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(skip)]
    pub private_config: Option<crate::configuration::NodeConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, TS)]
#[serde(deny_unknown_fields)]
pub struct AgentInfo {
    pub version: String,
    pub os: String,
    pub arch: String,
}
impl AgentInfo {
    pub fn current() -> Self {
        Self {
            version: crate::VERSION.into(),
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
        }
    }
    pub fn valid(&self) -> bool {
        [&self.version, &self.os, &self.arch].iter().all(|value| {
            !value.is_empty() && value.len() <= 64 && !value.chars().any(char::is_control)
        })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, TS)]
#[serde(tag = "status", content = "data", rename_all = "snake_case")]
pub enum TaskOutcome {
    Succeeded(AgentInfo),
    ServiceStatus(crate::ServiceReport),
    Unavailable { reason: crate::ServiceError },
    Xray(crate::XrayResult),
    ConfigApplied(crate::configuration::ConfigRef),
    Expired,
    Interrupted,
}

impl TaskOutcome {
    pub fn is_success(&self) -> bool {
        matches!(
            self,
            Self::Succeeded(_)
                | Self::ServiceStatus(_)
                | Self::Xray(crate::XrayResult::Ok(_))
                | Self::ConfigApplied(_)
        )
    }
    pub fn valid_for(&self, operation: &TaskOperation) -> bool {
        match (operation, self) {
            (TaskOperation::ApplyConfig(expected), Self::ConfigApplied(actual)) => {
                expected == actual
            }
            (TaskOperation::ApplyConfig(_), Self::Xray(crate::XrayResult::Error(_))) => true,
            (TaskOperation::Xray(operation), Self::Xray(crate::XrayResult::Ok(report))) => {
                report.valid_for(operation)
            }
            (TaskOperation::Xray(_), Self::Xray(crate::XrayResult::Error(_))) => true,
            (TaskOperation::AgentStatus, Self::Succeeded(info)) => info.valid(),
            (TaskOperation::ServiceStatus(query), Self::ServiceStatus(report)) => {
                query.service == report.service && report.valid()
            }
            (TaskOperation::ServiceStatus(_), Self::Unavailable { .. })
            | (_, Self::Expired | Self::Interrupted) => true,
            _ => false,
        }
    }
}

#[derive(Deserialize, Serialize, TS)]
pub struct TaskRecord {
    pub id: String,
    pub node_id: String,
    pub operation: TaskOperation,
    pub status: String,
    pub attempt_count: i32,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number")]
    pub deadline_at: i64,
    #[ts(type = "number | null")]
    pub finished_at: Option<i64>,
    pub result: Option<TaskOutcome>,
}

// No Debug: enrollment credentials must never enter diagnostic formatting.
#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", content = "payload", rename_all = "snake_case")]
pub enum Message {
    Enrolled {
        credential: String,
    },
    Hello(AgentInfo),
    Welcome,
    Retire,
    Retired,
    Heartbeat,
    Metrics(crate::MetricsSnapshot),
    Traffic(crate::traffic::TrafficReport),
    Task(TaskCommand),
    TaskAck {
        task_id: String,
    },
    TaskResult {
        task_id: String,
        result: TaskOutcome,
    },
    ResultAck {
        task_id: String,
    },
}

#[derive(Deserialize, Serialize)]
pub struct Envelope {
    pub version: u16,
    pub message_id: String,
    pub node_id: String,
    pub sent_at: String,
    #[serde(flatten)]
    pub message: Message,
}

impl Envelope {
    pub fn new(node_id: &str, message: Message) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            message_id: Uuid::new_v4().to_string(),
            node_id: node_id.into(),
            sent_at: OffsetDateTime::now_utc()
                .format(&Rfc3339)
                .expect("UTC timestamp formats"),
            message,
        }
    }
    pub fn parse(input: &str, node_id: Option<&str>) -> Result<Self, &'static str> {
        if input.len() > MAX_MESSAGE_BYTES {
            return Err("message_too_large");
        }
        let value: Self = serde_json::from_str(input).map_err(|_| "invalid_message")?;
        if value.version != PROTOCOL_VERSION {
            return Err("unsupported_version");
        }
        if Uuid::parse_str(&value.message_id).is_err()
            || Uuid::parse_str(&value.node_id).is_err()
            || node_id.is_some_and(|id| id != value.node_id)
        {
            return Err("identity_mismatch");
        }
        let timestamp =
            OffsetDateTime::parse(&value.sent_at, &Rfc3339).map_err(|_| "invalid_timestamp")?;
        if timestamp.offset() != UtcOffset::UTC
            || (OffsetDateTime::now_utc() - timestamp)
                .whole_seconds()
                .abs()
                > 300
        {
            return Err("clock_skew");
        }
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protocol_is_versioned_bounded_and_identity_checked() {
        let node = Uuid::new_v4().to_string();
        let mut frame = Envelope::new(&node, Message::Heartbeat);
        let json = serde_json::to_string(&frame).unwrap();
        assert!(Envelope::parse(&json, Some(&node)).is_ok());
        assert!(Envelope::parse(&json, Some(&Uuid::new_v4().to_string())).is_err());
        frame.version = PROTOCOL_VERSION + 1;
        assert!(Envelope::parse(&serde_json::to_string(&frame).unwrap(), None).is_err());
        frame.version = PROTOCOL_VERSION;
        frame.sent_at = "2020-01-01T00:00:00Z".into();
        assert!(Envelope::parse(&serde_json::to_string(&frame).unwrap(), None).is_err());
        assert!(Envelope::parse(&"x".repeat(MAX_MESSAGE_BYTES + 1), None).is_err());
        assert!(serde_json::from_str::<TaskOperation>("\"shell\"").is_err());
    }
}
