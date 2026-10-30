use crate::web::{Effect, Invoke, db::Assignment};
use serde_json::Value;

pub fn native_users(inbound: &Value) -> Vec<&Value> {
    let s = &inbound["settings"];
    s.get("clients")
        .or_else(|| s.get("users"))
        .and_then(Value::as_array)
        .map(|a| a.iter().collect())
        .unwrap_or_default()
}
fn decode(s: &str) -> String {
    fn digit(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(a), Some(b)) = (digit(bytes[i + 1]), digit(bytes[i + 2])) {
                out.push(a * 16 + b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
pub fn configuration_write(r: &Invoke) -> bool {
    matches!(
        crate::web::endpoint(r).map(|e| e.kind),
        Ok(crate::api::Kind::PutConfig
            | crate::api::Kind::AddInbound
            | crate::api::Kind::RemoveInbound
            | crate::api::Kind::PatchInbound
            | crate::api::Kind::AddOutbound
            | crate::api::Kind::RemoveOutbound
            | crate::api::Kind::AddUser
            | crate::api::Kind::RemoveUser
            | crate::api::Kind::AddRules
            | crate::api::Kind::RemoveRule
            | crate::api::Kind::Override)
    )
}
pub fn effects(assignments: &[Assignment], agent: &str, r: &Invoke) -> Vec<Effect> {
    let segments = r.path.split('/').map(decode).collect::<Vec<_>>();
    let tag = segments.get(5).map(String::as_str);
    assignments
        .iter()
        .filter(|a| a.agent_id == agent && a.status != "revoked")
        .filter_map(|a| {
            let email = a.native_user["email"].as_str().unwrap_or_default();
            let candidate = if r.method == "PUT" && r.path == "/api/v1/xray/config" {
                let user = r.body["inbounds"]
                    .as_array()
                    .and_then(|list| list.iter().find(|i| i["tag"] == a.inbound_tag))
                    .and_then(|i| {
                        native_users(i).into_iter().find(|u| {
                            u["email"]
                                .as_str()
                                .is_some_and(|s| s.eq_ignore_ascii_case(email))
                        })
                    });
                Some(user.cloned())
            } else if segments.get(4).is_some_and(|s| s == "inbounds")
                && tag == Some(a.inbound_tag.as_str())
            {
                if r.method == "DELETE" && segments.len() == 6 {
                    Some(None)
                } else if r.method == "DELETE"
                    && segments.get(6).is_some_and(|s| s == "users")
                    && segments
                        .get(7)
                        .is_some_and(|s| s.eq_ignore_ascii_case(email))
                {
                    Some(None)
                } else if r.method == "POST"
                    && segments.len() == 7
                    && r.body["email"]
                        .as_str()
                        .is_some_and(|s| s.eq_ignore_ascii_case(email))
                {
                    Some(Some(r.body.clone()))
                } else if r.method == "PATCH" && segments.len() == 6 {
                    match r.body["operation"].as_str() {
                        Some("remove_user")
                            if r.body["email"]
                                .as_str()
                                .is_some_and(|s| s.eq_ignore_ascii_case(email)) =>
                        {
                            Some(None)
                        }
                        Some("add_user")
                            if r.body["user"]["email"]
                                .as_str()
                                .is_some_and(|s| s.eq_ignore_ascii_case(email)) =>
                        {
                            Some(Some(r.body["user"].clone()))
                        }
                        _ => None,
                    }
                } else {
                    None
                }
            } else {
                None
            };
            candidate.and_then(|user| {
                let present = user.is_some();
                let value = user.unwrap_or_else(|| a.native_user.clone());
                Some(Effect {
                    id: a.id.clone(),
                    generation: a.generation + 1,
                    present,
                    native_user: value,
                })
            })
        })
        .collect()
}
