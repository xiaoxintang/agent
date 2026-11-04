use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ts_rs::TS;
use uuid::Uuid;

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConfigClient {
    pub id: String,
    pub protocol_id: String,
}
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConfigInbound {
    pub id: String,
    pub listen_address: String,
    pub listen_port: u16,
    pub target: String,
    pub server_names: Vec<String>,
    pub private_key: String,
    pub short_ids: Vec<String>,
    pub clients: Vec<ConfigClient>,
}
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NodeConfig {
    pub inbounds: Vec<ConfigInbound>,
}
impl std::fmt::Debug for NodeConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NodeConfig { credentials: [REDACTED] }")
    }
}
impl NodeConfig {
    pub fn render(&self) -> Result<Vec<u8>, &'static str> {
        if self.inbounds.len() > 16 {
            return Err("inbound_limit");
        }
        let mut ports = std::collections::BTreeSet::new();
        let mut ids = std::collections::BTreeSet::new();
        let mut count = 0;
        let mut inbounds = Vec::new();
        for inbound in &self.inbounds {
            if Uuid::parse_str(&inbound.id).is_err()
                || !ids.insert(&inbound.id)
                || inbound.listen_address.parse::<std::net::IpAddr>().is_err()
                || inbound.listen_port == 0
                || inbound.listen_port == crate::traffic::STATS_PORT
                || !ports.insert(inbound.listen_port)
            {
                return Err("invalid_listen_or_duplicate_port");
            }
            let (target, port) = inbound
                .target
                .rsplit_once(':')
                .ok_or("invalid_reality_target")?;
            if !domain(target)
                || port.parse::<u16>().ok().is_none_or(|value| value == 0)
                || inbound.server_names.is_empty()
                || inbound.server_names.len() > 5
                || !inbound.server_names.iter().all(|value| domain(value))
            {
                return Err("invalid_reality_target");
            }
            if !valid_key(&inbound.private_key) {
                return Err("invalid_reality_key");
            }
            if inbound.short_ids.is_empty()
                || inbound.short_ids.len() > 8
                || !inbound.short_ids.iter().all(|value| {
                    (2..=16).contains(&value.len())
                        && value.len() % 2 == 0
                        && value.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
            {
                return Err("invalid_short_id");
            }
            count += inbound.clients.len();
            if count > 256 {
                return Err("client_assignment_limit");
            }
            let mut clients = Vec::new();
            let mut unique = std::collections::BTreeSet::new();
            for client in &inbound.clients {
                if Uuid::parse_str(&client.id).is_err()
                    || Uuid::parse_str(&client.protocol_id).is_err()
                    || !unique.insert(&client.id)
                {
                    return Err("invalid_client");
                }
                clients.push(serde_json::json!({"id":client.protocol_id,"email":client.id,"flow":"xtls-rprx-vision","level":0}));
            }
            inbounds.push(serde_json::json!({"tag":inbound.id,"listen":inbound.listen_address,"port":inbound.listen_port,"protocol":"vless","settings":{"clients":clients,"decryption":"none"},"streamSettings":{"network":"tcp","security":"reality","realitySettings":{"show":false,"target":inbound.target,"xver":0,"serverNames":inbound.server_names,"privateKey":inbound.private_key,"shortIds":inbound.short_ids}}}));
        }
        inbounds.push(serde_json::json!({"tag":"panel-stats-in","listen":"127.0.0.1","port":crate::traffic::STATS_PORT,"protocol":"dokodemo-door","settings":{"address":"127.0.0.1"}}));
        let bytes = serde_json::to_vec(&serde_json::json!({
            "log":{"loglevel":"warning"},"inbounds":inbounds,"outbounds":[{"protocol":"freedom","tag":"direct"},{"protocol":"blackhole","tag":"panel-blocked"}],
            "stats":{},"api":{"tag":"panel-stats","services":["StatsService"]},
            "policy":{"levels":{"0":{"statsUserUplink":true,"statsUserDownlink":true}}},
            "routing":{"domainStrategy":"IPOnDemand","rules":[{"type":"field","inboundTag":["panel-stats-in"],"outboundTag":"panel-stats"},{"type":"field","ip":["127.0.0.0/8","::1/128"],"outboundTag":"panel-blocked"}]}
        })).map_err(|_| "serialization_failed")?;
        if bytes.len() > 48 * 1024 {
            return Err("configuration_too_large");
        }
        Ok(bytes)
    }
    pub fn content_hash(&self) -> Result<String, &'static str> {
        Ok(hex::encode(Sha256::digest(self.render()?)))
    }
}
pub fn valid_key(value: &str) -> bool {
    URL_SAFE_NO_PAD
        .decode(value)
        .ok()
        .is_some_and(|bytes| bytes.len() == 32 && bytes.iter().any(|byte| *byte != 0))
}
pub fn domain(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, TS)]
#[serde(deny_unknown_fields)]
pub struct ConfigRef {
    #[ts(type = "number")]
    pub revision: i64,
    pub content_hash: String,
}
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConfigSubmission {
    pub reference: ConfigRef,
    pub config: NodeConfig,
}
impl std::fmt::Debug for ConfigSubmission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigSubmission")
            .field("reference", &self.reference)
            .finish_non_exhaustive()
    }
}
#[derive(Deserialize, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct InboundInput {
    pub name: String,
    pub listen_address: String,
    pub listen_port: u16,
    pub target: String,
    pub server_names: Vec<String>,
    pub short_ids: Vec<String>,
    pub enabled: bool,
}
#[derive(Deserialize, Serialize, TS)]
pub struct InboundView {
    pub id: String,
    pub node_id: String,
    pub name: String,
    pub listen_address: String,
    pub listen_port: u16,
    pub target: String,
    pub server_names: Vec<String>,
    pub short_ids: Vec<String>,
    pub public_key: String,
    pub enabled: bool,
    pub client_ids: Vec<String>,
}
#[derive(Deserialize, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct ClientInput {
    pub display_name: String,
    pub enabled: bool,
    #[serde(default)]
    #[ts(optional, type = "number | null")]
    pub expiry_at: Option<i64>,
    #[serde(default)]
    #[ts(optional, type = "string | null")]
    pub quota_bytes: Option<String>,
}
#[derive(Deserialize, Serialize, TS)]
pub struct ClientView {
    #[ts(type = "number | null")]
    pub expiry_at: Option<i64>,
    pub quota_bytes: Option<String>,
    pub used_bytes: String,
    pub policy_blocked: bool,
    pub id: String,
    pub display_name: String,
    pub enabled: bool,
    #[ts(type = "number")]
    pub created_at: i64,
}
#[derive(Deserialize, Serialize, TS)]
pub struct CreatedClient {
    pub client: ClientView,
    pub protocol_id: String,
}
#[derive(Deserialize, Serialize, TS)]
pub struct RevisionView {
    pub id: String,
    pub node_id: String,
    #[ts(type = "number")]
    pub revision: i64,
    pub content_hash: String,
    pub status: String,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number | null")]
    pub applied_at: Option<i64>,
    pub task_id: Option<String>,
}
#[derive(Deserialize, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct ApplyInput {
    pub idempotency_key: String,
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn renders_real_vless_reality_and_rejects_unsafe_fields() {
        let mut config = NodeConfig {
            inbounds: vec![ConfigInbound {
                id: Uuid::new_v4().to_string(),
                listen_address: "127.0.0.1".into(),
                listen_port: 38443,
                target: "www.example.com:443".into(),
                server_names: vec!["www.example.com".into()],
                private_key: URL_SAFE_NO_PAD.encode([7; 32]),
                short_ids: vec!["abcd".into()],
                clients: vec![ConfigClient {
                    id: Uuid::new_v4().to_string(),
                    protocol_id: Uuid::new_v4().to_string(),
                }],
            }],
        };
        let value: serde_json::Value = serde_json::from_slice(&config.render().unwrap()).unwrap();
        assert_eq!(value["inbounds"][0]["protocol"], "vless");
        assert_eq!(
            value["inbounds"][0]["streamSettings"]["security"],
            "reality"
        );
        assert!(!format!("{config:?}").contains(&config.inbounds[0].private_key));
        config.inbounds[0].short_ids = vec!["../".into()];
        assert!(config.render().is_err());
    }
}
