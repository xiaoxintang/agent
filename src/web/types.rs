use super::db::Assignment;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Invoke {
    pub method: String,
    pub path: String,
    #[serde(default)]
    pub query: BTreeMap<String, String>,
    #[serde(default)]
    pub body: Value,
    #[serde(default)]
    pub confirmation: Option<String>,
    #[serde(default)]
    pub expected_revision: Option<i64>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Effect {
    pub id: String,
    pub generation: i64,
    pub present: bool,
    pub native_user: Value,
}
impl From<&Assignment> for Effect {
    fn from(a: &Assignment) -> Self {
        Self {
            id: a.id.clone(),
            generation: a.generation,
            present: a.desired_present,
            native_user: a.native_user.clone(),
        }
    }
}
pub struct Task {
    pub id: String,
    pub agent_id: String,
    pub request: Invoke,
    pub effects: Vec<Effect>,
    pub attempts: u32,
    pub uncertain: bool,
}
pub fn segment(s: &str) -> String {
    s.as_bytes()
        .iter()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(b) {
                (*b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
