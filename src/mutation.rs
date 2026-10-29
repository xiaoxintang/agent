//! 将业务变更应用到原生 Xray JSON 副本，再从官方编译结果提取对应的 gRPC 请求。
//! 本模块只构造数据，不执行网络、持久化或重启操作。

use crate::{
    error::{Error, Result},
    store::Snapshot,
};
use serde_json::{Value, json};

#[derive(Clone, Debug)]
pub enum Change {
    AddInbound(Value),
    RemoveInbound(String),
    AddOutbound(Value),
    RemoveOutbound(String),
    AddUser { tag: String, user: Value },
    RemoveUser { tag: String, email: String },
    AddRules { routing: Value, append: bool },
    RemoveRule(String),
    Override { tag: String, target: String },
}

pub struct Candidate {
    pub config: Value,
    pub overrides: Value,
}

fn list<'a>(root: &'a mut Value, key: &str) -> Result<&'a mut Vec<Value>> {
    if !root.is_object() {
        return Err(Error::Validation(
            "configuration section must be an object".into(),
        ));
    }
    if root.get(key).is_none() {
        root[key] = json!([]);
    }
    root.get_mut(key)
        .and_then(Value::as_array_mut)
        .ok_or_else(|| Error::Validation(format!("{key} must be an array")))
}

fn tag(value: &Value) -> Result<&str> {
    value
        .get("tag")
        .and_then(Value::as_str)
        .filter(|tag| !tag.is_empty())
        .ok_or_else(|| Error::BadRequest("a nonempty tag is required".into()))
}

fn add(root: &mut Value, key: &str, item: &Value) -> Result<()> {
    let item_tag = tag(item)?;
    let entries = list(root, key)?;
    if entries
        .iter()
        .any(|entry| entry.get("tag").and_then(Value::as_str) == Some(item_tag))
    {
        return Err(Error::Conflict(format!(
            "{key} tag already exists: {item_tag}"
        )));
    }
    entries.push(item.clone());
    Ok(())
}

fn remove(root: &mut Value, key: &str, wanted: &str, field: &str) -> Result<()> {
    let entries = list(root, key)?;
    let before = entries.len();
    entries.retain(|entry| entry.get(field).and_then(Value::as_str) != Some(wanted));
    if entries.len() == before {
        return Err(Error::NotFound(format!("{key} entry not found: {wanted}")));
    }
    Ok(())
}

fn inbound_mut<'a>(config: &'a mut Value, wanted: &str) -> Result<&'a mut Value> {
    list(config, "inbounds")?
        .iter_mut()
        .find(|entry| entry.get("tag").and_then(Value::as_str) == Some(wanted))
        .ok_or_else(|| Error::NotFound(format!("inbound not found: {wanted}")))
}

fn users_mut(inbound: &mut Value) -> Result<&mut Vec<Value>> {
    // 各协议原生 JSON 的用户容器不同；WireGuard 使用 peers，其他支持的协议使用用户数组。
    let protocol = inbound
        .get("protocol")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let default_key = match protocol.as_str() {
        "vmess" | "vless" | "trojan" | "hysteria" | "masque" => "clients",
        "shadowsocks" => "clients",
        "wireguard" => "peers",
        _ => {
            return Err(Error::Unsupported(format!(
                "dynamic users are not supported for inbound protocol {protocol}"
            )));
        }
    };
    if inbound.get("settings").is_none() {
        inbound["settings"] = json!({});
    }
    let settings = inbound.get_mut("settings").unwrap();
    if default_key == "peers" {
        return list(settings, "peers");
    }
    if protocol == "shadowsocks"
        && settings.get("clients").is_none()
        && settings.get("users").is_none()
    {
        if settings
            .get("method")
            .and_then(Value::as_str)
            .is_some_and(|method| method.starts_with("2022-"))
        {
            // 单用户和多用户是不同的运行时实现，不能只靠 AddUser 切换实例类型。
            return Err(Error::Unsupported("a Shadowsocks 2022 single-user inbound cannot become multi-user through AlterInbound; create it with a clients array first".into()));
        }
        // 传统 Shadowsocks 从顶层凭据改为 clients 时保留原用户，避免重启后原凭据消失。
        let mut existing = serde_json::Map::new();
        for key in ["method", "password", "email", "level"] {
            if let Some(value) = settings.get(key) {
                existing.insert(key.into(), value.clone());
            }
        }
        if existing.contains_key("password") {
            settings["clients"] = json!([existing]);
        }
    }
    // Upstream aliases prefer clients over users. Preserve the effective source field.
    let key = if settings.get("clients").is_some() {
        "clients"
    } else if settings.get("users").is_some() {
        "users"
    } else {
        default_key
    };
    list(settings, key)
}

fn email(user: &Value) -> Result<&str> {
    user.get("email")
        .and_then(Value::as_str)
        .filter(|email| !email.is_empty())
        .ok_or_else(|| Error::BadRequest("dynamic users require a nonempty email".into()))
}

impl Change {
    pub fn method(&self) -> &'static str {
        match self {
            Self::AddInbound(_) => "HandlerService/AddInbound",
            Self::RemoveInbound(_) => "HandlerService/RemoveInbound",
            Self::AddOutbound(_) => "HandlerService/AddOutbound",
            Self::RemoveOutbound(_) => "HandlerService/RemoveOutbound",
            Self::AddUser { .. } | Self::RemoveUser { .. } => "HandlerService/AlterInbound",
            Self::AddRules { .. } => "RoutingService/AddRule",
            Self::RemoveRule(_) => "RoutingService/RemoveRule",
            Self::Override { .. } => "RoutingService/OverrideBalancerTarget",
        }
    }

    pub fn apply(&self, snapshot: &Snapshot) -> Result<Candidate> {
        // 只修改克隆的候选；是否提交由 App 在 RPC 结果确认后决定。
        let mut candidate = Candidate {
            config: snapshot.config.clone(),
            overrides: snapshot.overrides.clone(),
        };
        match self {
            Self::AddInbound(value) => add(&mut candidate.config, "inbounds", value)?,
            Self::RemoveInbound(tag) => remove(&mut candidate.config, "inbounds", tag, "tag")?,
            Self::AddOutbound(value) => add(&mut candidate.config, "outbounds", value)?,
            Self::RemoveOutbound(tag) => remove(&mut candidate.config, "outbounds", tag, "tag")?,
            Self::AddUser { tag, user } => {
                let wanted = email(user)?;
                let inbound = inbound_mut(&mut candidate.config, tag)?;
                let shadowsocks_2022 = inbound.get("protocol").and_then(Value::as_str)
                    == Some("shadowsocks")
                    && inbound
                        .pointer("/settings/method")
                        .and_then(Value::as_str)
                        .is_some_and(|method| method.starts_with("2022-"));
                let users = users_mut(inbound)?;
                if shadowsocks_2022 && users.is_empty() {
                    return Err(Error::Unsupported(
                        "Shadowsocks 2022 dynamic users require an existing multi-user inbound"
                            .into(),
                    ));
                }
                if users.iter().any(|entry| {
                    entry
                        .get("email")
                        .and_then(Value::as_str)
                        .is_some_and(|entry| entry.eq_ignore_ascii_case(wanted))
                }) {
                    return Err(Error::Conflict("user email already exists".into()));
                }
                users.push(user.clone());
            }
            Self::RemoveUser { tag, email } => {
                let inbound = inbound_mut(&mut candidate.config, tag)?;
                let shadowsocks_2022 = inbound.get("protocol").and_then(Value::as_str)
                    == Some("shadowsocks")
                    && inbound
                        .pointer("/settings/method")
                        .and_then(Value::as_str)
                        .is_some_and(|method| method.starts_with("2022-"));
                // WireGuard 的用户标识按原字符串匹配，不能套用其他协议的大小写规则。
                let case_sensitive =
                    inbound.get("protocol").and_then(Value::as_str) == Some("wireguard");
                let users = users_mut(inbound)?;
                let before = users.len();
                users.retain(|entry| {
                    !entry
                        .get("email")
                        .and_then(Value::as_str)
                        .is_some_and(|entry| {
                            if case_sensitive {
                                entry == email
                            } else {
                                entry.eq_ignore_ascii_case(email)
                            }
                        })
                });
                if users.len() == before {
                    return Err(Error::NotFound("user not found".into()));
                }
                if shadowsocks_2022 && users.is_empty() {
                    // 空客户端数组会让启动配置退回单用户模式，与运行时删除结果不同。
                    return Err(Error::Validation("removing the final Shadowsocks 2022 client would turn the persisted configuration into a single-user server; remove the inbound instead".into()));
                }
            }
            Self::AddRules { routing, append } => {
                let object = routing
                    .as_object()
                    .ok_or_else(|| Error::BadRequest("routing must be an object".into()))?;
                if object
                    .keys()
                    .any(|key| !matches!(key.as_str(), "rules" | "balancers"))
                {
                    // 上游 AddRule 不会热更新这些全局字段，持久化它们会导致重启前后行为不一致。
                    return Err(Error::Validation("dynamic routing only accepts rules and balancers; global routing settings require configuration replacement".into()));
                }
                if candidate.config.get("routing").is_none() {
                    candidate.config["routing"] = json!({"rules":[]});
                }
                let current = &mut candidate.config["routing"];
                for key in ["rules", "balancers"] {
                    let additions = object.get(key).cloned().unwrap_or(json!([]));
                    let additions = additions.as_array().ok_or_else(|| {
                        Error::BadRequest(format!("routing.{key} must be an array"))
                    })?;
                    if *append {
                        list(current, key)?.extend(additions.iter().cloned());
                    } else {
                        current[key] = Value::Array(additions.clone());
                    }
                }
                // ReloadRules(false) recreates balancers and clears every runtime override.
                if !append {
                    candidate.overrides = json!({});
                }
            }
            Self::RemoveRule(tag) => {
                if candidate.config.get("routing").is_none() {
                    return Err(Error::NotFound("routing not configured".into()));
                }
                remove(&mut candidate.config["routing"], "rules", tag, "ruleTag")?;
            }
            Self::Override { tag, target } => {
                let exists = candidate
                    .config
                    .pointer("/routing/balancers")
                    .and_then(Value::as_array)
                    .is_some_and(|balancers| {
                        balancers
                            .iter()
                            .any(|entry| entry.get("tag").and_then(Value::as_str) == Some(tag))
                    });
                if !exists {
                    return Err(Error::NotFound("balancer not found".into()));
                }
                if target.is_empty() {
                    candidate.overrides.as_object_mut().unwrap().remove(tag);
                } else {
                    candidate.overrides[tag] = Value::String(target.clone());
                }
            }
        }
        Ok(candidate)
    }

    pub fn request(&self, compiled: &Value) -> Result<Value> {
        // compiled 来自当前 Xray 的官方转换，包含协议账户、传输和路由的准确 protobuf 表达。
        match self {
            Self::AddInbound(value) => {
                Ok(json!({"inbound":compiled_handler(compiled,"inbound",tag(value)?)?}))
            }
            Self::AddOutbound(value) => {
                Ok(json!({"outbound":compiled_handler(compiled,"outbound",tag(value)?)?}))
            }
            Self::RemoveInbound(tag) | Self::RemoveOutbound(tag) => Ok(json!({"tag":tag})),
            Self::AddUser { tag, user } => {
                // 从完整候选中取编译后的 User，避免在 Rust 中手写每种协议的账户转换。
                let inbound = compiled_handler(compiled, "inbound", tag)?;
                let settings = inbound.pointer("/proxySettings/value").ok_or_else(|| {
                    Error::Unsupported("compiled inbound has no proxy settings".into())
                })?;
                let wanted = email(user)?;
                let found = ["users", "user"]
                    .into_iter()
                    .filter_map(|key| settings.get(key).and_then(Value::as_array))
                    .flatten()
                    .find(|entry| {
                        entry
                            .get("email")
                            .and_then(Value::as_str)
                            .is_some_and(|email| email.eq_ignore_ascii_case(wanted))
                    })
                    .ok_or_else(|| {
                        Error::Unsupported(
                            "protocol does not expose a persistable common User".into(),
                        )
                    })?;
                Ok(
                    json!({"tag":tag,"operation":{"type":"xray.app.proxyman.command.AddUserOperation","value":{"user":found}}}),
                )
            }
            Self::RemoveUser { tag, email } => {
                let settings =
                    compiled_handler(compiled, "inbound", tag)?.pointer("/proxySettings/value");
                // 部分原生配置会回退到顶层默认凭据，必须拒绝会在重启后重新创建用户的删除。
                if settings.is_some_and(|settings| {
                    ["users", "user"]
                        .iter()
                        .filter_map(|key| settings.get(*key).and_then(Value::as_array))
                        .flatten()
                        .any(|user| {
                            user.get("email")
                                .and_then(Value::as_str)
                                .is_some_and(|actual| actual.eq_ignore_ascii_case(email))
                        })
                }) {
                    return Err(Error::Validation("native configuration would recreate the removed user; remove the inbound or change its base credentials instead".into()));
                }
                Ok(
                    json!({"tag":tag,"operation":{"type":"xray.app.proxyman.command.RemoveUserOperation","value":{"email":email}}}),
                )
            }
            Self::AddRules { routing, append } => {
                let mut config = compiled
                    .get("app")
                    .and_then(Value::as_array)
                    .and_then(|apps| {
                        apps.iter().find(|app| {
                            app.get("type").and_then(Value::as_str)
                                == Some("xray.app.router.Config")
                        })
                    })
                    .ok_or_else(|| {
                        Error::Validation("compiled routing configuration not found".into())
                    })?
                    .clone();
                // The full candidate validates references to existing balancers. Only send the
                // requested suffix to ReloadRules, or append would duplicate existing rules.
                for (native, protobuf) in [("rules", "rule"), ("balancers", "balancingRule")] {
                    let count = routing
                        .get(native)
                        .and_then(Value::as_array)
                        .map_or(0, Vec::len);
                    let entries = config["value"]
                        .get(protobuf)
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    if entries.len() < count {
                        return Err(Error::Validation(
                            "compiled routing delta lost entries".into(),
                        ));
                    }
                    config["value"][protobuf] =
                        Value::Array(entries[entries.len() - count..].to_vec());
                }
                Ok(json!({"config":config,"shouldAppend":append}))
            }
            Self::RemoveRule(tag) => Ok(json!({"ruleTag":tag})),
            Self::Override { tag, target } => Ok(json!({"balancerTag":tag,"target":target})),
        }
    }
}

fn compiled_handler<'a>(compiled: &'a Value, key: &str, tag: &str) -> Result<&'a Value> {
    compiled
        .get(key)
        .and_then(Value::as_array)
        .and_then(|entries| {
            entries
                .iter()
                .find(|entry| entry.get("tag").and_then(Value::as_str) == Some(tag))
        })
        .ok_or_else(|| Error::Validation(format!("compiled {key} not found: {tag}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot() -> Snapshot {
        Snapshot {
            revision: 1,
            config: json!({"inbounds":[{"tag":"vless","protocol":"vless","settings":{"clients":[{"email":"first@example.com","id":"id"}]}}],"routing":{"domainStrategy":"IPIfNonMatch","rules":[{"ruleTag":"first","outboundTag":"direct"}],"balancers":[{"tag":"balance","selector":["direct"]}]}}),
            overrides: json!({"balance":"direct"}),
        }
    }

    #[test]
    fn users_update_the_effective_json_array_without_losing_settings() {
        let old = snapshot();
        let change = Change::AddUser {
            tag: "vless".into(),
            user: json!({"email":"new@example.com","id":"new"}),
        };
        let candidate = change.apply(&old).unwrap();
        assert_eq!(
            candidate
                .config
                .pointer("/inbounds/0/settings/clients")
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(candidate.config["routing"], old.config["routing"]);
        assert!(
            Change::AddUser {
                tag: "vless".into(),
                user: json!({"email":"FIRST@example.com"})
            }
            .apply(&old)
            .is_err()
        );
    }

    #[test]
    fn replacing_routes_preserves_global_settings_but_clears_overrides() {
        let candidate = Change::AddRules {
            routing: json!({"rules":[]}),
            append: false,
        }
        .apply(&snapshot())
        .unwrap();
        assert_eq!(
            candidate.config["routing"]["domainStrategy"],
            "IPIfNonMatch"
        );
        assert_eq!(candidate.config["routing"]["rules"], json!([]));
        assert_eq!(candidate.overrides, json!({}));
        assert!(
            Change::AddRules {
                routing: json!({"domainStrategy":"AsIs"}),
                append: true
            }
            .apply(&snapshot())
            .is_err()
        );
    }
}
