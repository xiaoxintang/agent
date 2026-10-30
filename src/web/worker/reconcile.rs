use super::native_users;
use crate::{
    error::Result,
    web::{Task, Web},
};
use serde_json::{Value, json};

fn subset(expected: &Value, actual: &Value) -> bool {
    match (expected, actual) {
        (Value::Object(e), Value::Object(a)) => e
            .iter()
            .all(|(k, v)| a.get(k).is_some_and(|x| subset(v, x))),
        (Value::Array(e), Value::Array(a)) => {
            e.len() == a.len() && e.iter().zip(a).all(|(x, y)| subset(x, y))
        }
        _ => expected == actual,
    }
}
pub(super) async fn applied(web: &Web, task: &Task) -> Result<bool> {
    let r = &task.request;
    let snapshot = web.read(&task.agent_id, "/api/v1/xray/config").await?;
    let config = &snapshot["config"];
    let parts = r.path.split('/').map(decode).collect::<Vec<_>>();
    let kind = crate::web::endpoint(r)?.kind;
    use crate::api::Kind;
    Ok(match kind {
        Kind::PutConfig => config == &r.body,
        Kind::AddInbound | Kind::AddOutbound => {
            let list = if kind == Kind::AddInbound {
                "inbounds"
            } else {
                "outbounds"
            };
            config[list].as_array().is_some_and(|items| {
                items
                    .iter()
                    .any(|i| i["tag"] == r.body["tag"] && subset(&r.body, i))
            })
        }
        Kind::RemoveInbound | Kind::RemoveOutbound => {
            let list = if kind == Kind::RemoveInbound {
                "inbounds"
            } else {
                "outbounds"
            };
            config[list].as_array().is_some_and(|items| {
                !items
                    .iter()
                    .any(|i| i["tag"].as_str() == parts.get(5).map(String::as_str))
            })
        }
        Kind::AddUser | Kind::RemoveUser | Kind::PatchInbound => {
            let add = kind == Kind::AddUser
                || (kind == Kind::PatchInbound && r.body["operation"] == "add_user");
            let user = if kind == Kind::PatchInbound {
                &r.body["user"]
            } else {
                &r.body
            };
            let email = if add {
                user["email"].as_str().unwrap_or_default()
            } else if kind == Kind::PatchInbound {
                r.body["email"].as_str().unwrap_or_default()
            } else {
                parts.get(7).map(String::as_str).unwrap_or_default()
            };
            let inbound = config["inbounds"].as_array().and_then(|items| {
                items
                    .iter()
                    .find(|i| i["tag"].as_str() == parts.get(5).map(String::as_str))
            });
            if let Some(inbound) = inbound {
                let found = native_users(inbound).into_iter().find(|u| {
                    u["email"]
                        .as_str()
                        .is_some_and(|s| s.eq_ignore_ascii_case(email))
                });
                let committed = if add {
                    found.is_some_and(|u| subset(user, u))
                } else {
                    found.is_none()
                };
                if !committed {
                    false
                } else {
                    let live = web
                        .read(
                            &task.agent_id,
                            &format!(
                                "/api/v1/xray/inbounds/{}/users",
                                crate::web::segment(
                                    parts.get(5).map(String::as_str).unwrap_or_default()
                                )
                            ),
                        )
                        .await?;
                    let live_user = live["users"].as_array().and_then(|users| {
                        users.iter().find(|u| {
                            u["email"]
                                .as_str()
                                .is_some_and(|s| s.eq_ignore_ascii_case(email))
                        })
                    });
                    if add {
                        live_user.is_some_and(|actual| runtime_credentials_match(user, actual))
                    } else {
                        live_user.is_none()
                    }
                }
            } else {
                !add
            }
        }
        Kind::AddRules => {
            let routing = &r.body["routing"];
            if r.body["shouldAppend"] == true {
                ["rules", "balancers"].iter().all(|key| {
                    routing[*key].as_array().is_none_or(|expected| {
                        expected.iter().all(|e| {
                            config["routing"][*key]
                                .as_array()
                                .is_some_and(|a| a.contains(e))
                        })
                    })
                })
            } else {
                ["rules", "balancers"].iter().all(|k| {
                    routing.get(*k).unwrap_or(&json!([]))
                        == config["routing"].get(*k).unwrap_or(&json!([]))
                })
            }
        }
        Kind::RemoveRule => config["routing"]["rules"].as_array().is_none_or(|items| {
            !items
                .iter()
                .any(|i| i["ruleTag"].as_str() == parts.get(6).map(String::as_str))
        }),
        Kind::Override => {
            let tag = parts.get(6).map(String::as_str).unwrap_or_default();
            let expected = r.body["target"].as_str().unwrap_or_default();
            if expected.is_empty() {
                snapshot["overrides"].get(tag).is_none()
            } else {
                snapshot["overrides"][tag] == expected
            }
        }
        _ => false,
    })
}

pub(super) fn runtime_credentials_match(expected: &Value, actual: &Value) -> bool {
    let account = actual
        .get("account")
        .and_then(|a| a.get("value"))
        .unwrap_or(actual);
    [
        "id",
        "password",
        "auth",
        "pass",
        "flow",
        "publicKey",
        "preSharedKey",
    ]
    .iter()
    .all(|key| {
        expected.get(*key).is_none_or(|value| {
            let found = account.get(*key).or_else(|| match *key {
                "publicKey" => account.get("public_key"),
                "preSharedKey" => account.get("pre_shared_key"),
                _ => None,
            });
            found == Some(value) || (value == "" && found.is_none())
        })
    })
}
