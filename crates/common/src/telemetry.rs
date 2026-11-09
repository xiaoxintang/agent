use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub const METRICS_SECONDS: u64 = 5;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, TS)]
#[serde(deny_unknown_fields)]
pub struct ByteUsage {
    pub total_bytes: String,
    pub used_bytes: String,
    pub available_bytes: String,
}
impl ByteUsage {
    pub fn valid(&self) -> bool {
        match (
            bytes(&self.total_bytes),
            bytes(&self.used_bytes),
            bytes(&self.available_bytes),
        ) {
            (Some(total), Some(used), Some(available)) => {
                total > 0
                    && used <= total
                    && available <= total
                    && used.checked_add(available).is_some_and(|sum| sum <= total)
            }
            _ => false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, TS)]
#[serde(deny_unknown_fields)]
pub struct NetworkUsage {
    pub received_bytes: String,
    pub transmitted_bytes: String,
    pub received_bytes_per_second: Option<f64>,
    pub transmitted_bytes_per_second: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, TS)]
#[serde(rename_all = "snake_case")]
pub enum MetricName {
    Cpu,
    Memory,
    Disk,
    Network,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, TS)]
#[serde(rename_all = "snake_case")]
pub enum MetricReason {
    UnsupportedPlatform,
    ReadFailed,
    InvalidData,
    LimitExceeded,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, TS)]
#[serde(deny_unknown_fields)]
pub struct MetricIssue {
    pub metric: MetricName,
    pub reason: MetricReason,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, TS)]
#[serde(deny_unknown_fields)]
pub struct MetricsSnapshot {
    #[ts(type = "number")]
    pub sampled_at: i64,
    pub cpu_percent: Option<f64>,
    pub memory: Option<ByteUsage>,
    pub disk: Option<ByteUsage>,
    pub network: Option<NetworkUsage>,
    pub issues: Vec<MetricIssue>,
}
impl MetricsSnapshot {
    pub fn valid(&self, now: i64) -> bool {
        self.sampled_at.abs_diff(now) <= 300
            && self.issues.len() <= 4
            && self.issues.iter().enumerate().all(|(index, issue)| {
                !self.issues[..index]
                    .iter()
                    .any(|previous| previous.metric == issue.metric)
            })
            && self
                .cpu_percent
                .is_none_or(|value| value.is_finite() && (0.0..=100.0).contains(&value))
            && self.memory.as_ref().is_none_or(ByteUsage::valid)
            && self.disk.as_ref().is_none_or(ByteUsage::valid)
            && self.network.as_ref().is_none_or(|network| {
                bytes(&network.received_bytes).is_some()
                    && bytes(&network.transmitted_bytes).is_some()
                    && [
                        network.received_bytes_per_second,
                        network.transmitted_bytes_per_second,
                    ]
                    .iter()
                    .all(|rate| rate.is_none_or(|value| value.is_finite() && value >= 0.0))
            })
    }
}

fn bytes(value: &str) -> Option<u64> {
    if value.is_empty() || value.len() > 20 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

#[derive(Deserialize, Serialize, TS)]
pub struct NodeMetrics {
    pub node_id: String,
    pub snapshot: Option<MetricsSnapshot>,
    #[ts(type = "number | null")]
    pub received_at: Option<i64>,
    pub stale: bool,
}

#[derive(Deserialize, Serialize, TS)]
pub struct NodeMetricsSummary {
    pub cpu_percent: Option<f64>,
    pub memory: Option<ByteUsage>,
    #[ts(type = "number | null")]
    pub received_at: Option<i64>,
    pub stale: bool,
}

#[derive(Deserialize, Serialize, TS)]
pub struct NodeSummary {
    pub node: crate::Node,
    pub metrics: NodeMetricsSummary,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, TS)]
#[serde(rename_all = "snake_case")]
pub enum ManagedService {
    PanelAgent,
    Xray,
}
impl ManagedService {
    pub fn unit_name(&self) -> &'static str {
        match self {
            Self::PanelAgent => "panel-agent.service",
            Self::Xray => "xray.service",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, TS)]
#[serde(deny_unknown_fields)]
pub struct ServiceReport {
    pub service: ManagedService,
    pub load_state: String,
    pub active_state: Option<String>,
    pub sub_state: Option<String>,
}
impl ServiceReport {
    pub fn valid(&self) -> bool {
        std::iter::once(Some(&self.load_state))
            .chain([self.active_state.as_ref(), self.sub_state.as_ref()])
            .all(|value| {
                value.is_none_or(|value| {
                    !value.is_empty() && value.len() <= 64 && !value.chars().any(char::is_control)
                })
            })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, TS)]
#[serde(rename_all = "snake_case")]
pub enum ServiceError {
    UnsupportedPlatform,
    SystemdUnavailable,
    AccessDenied,
    Timeout,
    PrivilegeDenied,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, TS)]
#[serde(deny_unknown_fields)]
pub struct ServiceQuery {
    pub service: ManagedService,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_invalid_metrics_and_unbounded_values() {
        let mut snapshot = MetricsSnapshot {
            sampled_at: 100,
            cpu_percent: Some(20.0),
            memory: None,
            disk: None,
            network: None,
            issues: vec![],
        };
        assert!(snapshot.valid(100));
        snapshot.cpu_percent = Some(f64::NAN);
        assert!(!snapshot.valid(100));
        snapshot.cpu_percent = Some(101.0);
        assert!(!snapshot.valid(100));
        snapshot.cpu_percent = None;
        snapshot.memory = Some(ByteUsage {
            total_bytes: "10".into(),
            used_bytes: "9".into(),
            available_bytes: "2".into(),
        });
        assert!(!snapshot.valid(100));
        assert!(serde_json::from_str::<ManagedService>("\"ssh.service\"").is_err());
        assert_eq!(
            ManagedService::PanelAgent.unit_name(),
            "panel-agent.service"
        );
        assert_eq!(ManagedService::Xray.unit_name(), "xray.service");
    }
    #[test]
    fn the_agent_wire_parser_rejects_unauthorized_systemd_operations_before_dispatch() {
        for operation in [
            r#"{"service_status":{"service":"ssh.service"}}"#,
            r#"{"service_status":{"service":"../../etc/passwd"}}"#,
            r#"{"service_status":{"service":"panel_agent","method":"RestartUnit"}}"#,
            r#"{"service_restart":{"service":"panel_agent"}}"#,
            r#"{"shell":"id"}"#,
        ] {
            assert!(
                serde_json::from_str::<crate::TaskOperation>(operation).is_err(),
                "{operation}"
            );
        }
        let operation: crate::TaskOperation =
            serde_json::from_str(r#"{"service_status":{"service":"xray"}}"#).unwrap();
        let report = ServiceReport {
            service: ManagedService::PanelAgent,
            load_state: "loaded".into(),
            active_state: Some("active".into()),
            sub_state: Some("running".into()),
        };
        assert!(!crate::TaskOutcome::ServiceStatus(report).valid_for(&operation));
        assert!(!crate::TaskOutcome::Succeeded(crate::AgentInfo::current()).valid_for(&operation));
    }
}
