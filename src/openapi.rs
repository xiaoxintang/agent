//! 合并业务 Rust 类型、原生 Xray JSON schema 和 protobuf 描述符，生成 OpenAPI 3.1。
//! 文档生成不读取主机配置或凭据，HTTP 和离线导出使用相同结果。

use crate::{
    api::{ApiComponents, ENDPOINTS, Kind},
    grpc,
};
use prost_reflect::{FieldDescriptor, Kind as ProtoKind, MessageDescriptor};
use serde_json::{Map, Value, json};
use utoipa::OpenApi;

fn reference(name: &str) -> Value {
    json!({"$ref":format!("#/components/schemas/{name}")})
}
fn protobuf_name(name: &str) -> String {
    format!("pb.{name}")
}

fn scalar(kind: ProtoKind) -> Value {
    // schema 遵守 Protobuf JSON 约定，例如 64 位整数用字符串、bytes 用 Base64。
    match kind {
        ProtoKind::Bool => json!({"type":"boolean"}),
        ProtoKind::String => json!({"type":"string"}),
        ProtoKind::Bytes => {
            json!({"type":"string","format":"byte","description":"Base64 encoded protobuf bytes"})
        }
        ProtoKind::Int64 | ProtoKind::Sint64 | ProtoKind::Sfixed64 => {
            json!({"type":"string","pattern":"^-?[0-9]+$","description":"Protobuf JSON represents int64 as decimal strings"})
        }
        ProtoKind::Uint64 | ProtoKind::Fixed64 => {
            json!({"type":"string","pattern":"^[0-9]+$","description":"Protobuf JSON represents uint64 as decimal strings"})
        }
        ProtoKind::Int32 | ProtoKind::Sint32 | ProtoKind::Sfixed32 => {
            json!({"type":"integer","format":"int32"})
        }
        ProtoKind::Uint32 | ProtoKind::Fixed32 => {
            json!({"type":"integer","minimum":0,"maximum":4294967295u64})
        }
        ProtoKind::Float | ProtoKind::Double => json!({"type":"number"}),
        ProtoKind::Message(message) => reference(&protobuf_name(message.full_name())),
        ProtoKind::Enum(enumeration) => {
            json!({"oneOf":[{"type":"string","enum":enumeration.values().map(|value|value.name().to_owned()).collect::<Vec<_>>()},{"type":"integer"}]})
        }
    }
}

fn field_schema(field: &FieldDescriptor) -> Value {
    if field.is_map() {
        let ProtoKind::Message(entry) = field.kind() else {
            unreachable!()
        };
        // protobuf map 的合成 entry 不暴露为对象类型，直接描述成 JSON 字典。
        return json!({"type":"object","additionalProperties":scalar(entry.get_field_by_name("value").unwrap().kind())});
    }
    let value = scalar(field.kind());
    if field.is_list() {
        json!({"type":"array","items":value})
    } else {
        value
    }
}

fn protobuf_schema(message: &MessageDescriptor) -> Value {
    let properties: Map<String, Value> = message
        .fields()
        .map(|field| (field.json_name().to_owned(), field_schema(&field)))
        .collect();
    json!({"type":"object","description":format!("{}; standard Protobuf JSON with expanded TypedMessage values",message.full_name()),"properties":properties,"additionalProperties":false})
}

fn object(properties: Value, required: &[&str]) -> Value {
    json!({"type":"object","properties":properties,"required":required,"additionalProperties":true})
}

fn native_schemas(schemas: &mut Map<String, Value>) {
    // 写入接口接收普通 Xray JSON，不直接套用 core.Config 的 protobuf 字段结构。
    // 未穷举的协议扩展保留为 object，最终由安装的 Xray 版本验证。
    schemas.insert("NativeUser".into(),object(json!({
        "email":{"type":"string","description":"Nonempty identity used for dynamic removal"},
        "level":{"type":"integer","minimum":0},
        "id":{"type":"string","description":"VLESS/VMess UUID"},
        "flow":{"type":"string"},"password":{"type":"string","format":"password"},
        "auth":{"type":"string","format":"password","description":"Hysteria authentication"},
        "pass":{"type":"string","format":"password","description":"MASQUE password"},
        "publicKey":{"type":"string","description":"WireGuard peer public key"},
        "preSharedKey":{"type":"string","format":"password"},"allowedIPs":{"type":"array","items":{"type":"string"}},
        "keepAlive":{"type":"integer"}
    }),&["email"]));
    schemas.insert("ClientSettings".into(),object(json!({"clients":{"type":"array","items":reference("NativeUser")},"users":{"type":"array","items":reference("NativeUser")},"decryption":{"type":"string"},"method":{"type":"string"},"password":{"type":"string","format":"password"},"version":{"type":"integer"},"address":{"type":"array","items":{"type":"string"}},"mtu":{"type":"integer"}}),&[]));
    schemas.insert("StreamSettings".into(),object(json!({
        "network":{"type":"string","examples":["tcp","ws","grpc","xhttp","httpupgrade","hysteria","masque","xdrive"]},
        "security":{"type":"string","enum":["none","tls","reality"]},
        "tlsSettings":{"type":"object","properties":{"serverName":{"type":"string"},"alpn":{"type":"array","items":{"type":"string"}},"certificates":{"type":"array","items":{"type":"object","properties":{"certificateFile":{"type":"string"},"keyFile":{"type":"string"}}}}}},
        "realitySettings":{"type":"object","properties":{"target":{"type":"string"},"serverNames":{"type":"array","items":{"type":"string"}},"privateKey":{"type":"string","format":"password"},"publicKey":{"type":"string"},"shortIds":{"type":"array","items":{"type":"string"}}}},
        "wsSettings":{"type":"object","properties":{"path":{"type":"string"},"headers":{"type":"object","additionalProperties":{"type":"string"}}}},
        "grpcSettings":{"type":"object","properties":{"serviceName":{"type":"string"},"multiMode":{"type":"boolean"}}},
        "tcpSettings":{"type":"object"},"xhttpSettings":{"type":"object"},"kcpSettings":{"type":"object"},"httpupgradeSettings":{"type":"object"},"hysteriaSettings":{"type":"object"},"masqueSettings":{"type":"object"},"xdriveSettings":{"type":"object"},"sockopt":{"type":"object"},"finalmask":{"type":"object"}
    }),&[]));
    let inbound_protocols = [
        "dokodemo-door",
        "tunnel",
        "http",
        "shadowsocks",
        "socks",
        "mixed",
        "vless",
        "vmess",
        "trojan",
        "wireguard",
        "hysteria",
        "masque",
        "tun",
    ];
    let outbound_protocols = [
        "blackhole",
        "block",
        "loopback",
        "freedom",
        "direct",
        "http",
        "shadowsocks",
        "socks",
        "vless",
        "vmess",
        "trojan",
        "hysteria",
        "masque",
        "dns",
        "wireguard",
    ];
    for (name, protocols, inbound) in [
        ("InboundConfig", inbound_protocols.as_slice(), true),
        ("OutboundConfig", outbound_protocols.as_slice(), false),
    ] {
        // 以 protocol 区分入站和出站配置，客户端可据此展示正确的业务字段。
        let alternatives:Vec<_>=protocols.iter().map(|protocol| {
            let mut properties=json!({"tag":{"type":"string","minLength":1},"protocol":{"type":"string","enum":[protocol]},"streamSettings":reference("StreamSettings")});
            if inbound {
                properties["listen"]=json!({"type":"string"});
                properties["port"]=json!({"oneOf":[{"type":"integer","minimum":1,"maximum":65535},{"type":"string","description":"Port or port range"}]});
                properties["sniffing"]=json!({"type":"object","properties":{"enabled":{"type":"boolean"},"destOverride":{"type":"array","items":{"type":"string"}},"routeOnly":{"type":"boolean"}}});
                properties["settings"]=if matches!(*protocol,"vless"|"vmess"|"trojan"|"shadowsocks"|"hysteria"|"masque") {reference("ClientSettings")} else {json!({"type":"object","description":format!("Native Xray {protocol} inbound settings; validated by the installed Xray binary")})};
            } else {
                properties["sendThrough"]=json!({"type":"string"});
                properties["mux"]=json!({"type":"object","properties":{"enabled":{"type":"boolean"},"concurrency":{"type":"integer"}}});
                properties["settings"]=object(json!({"address":{"type":"string"},"port":{"type":"integer"},"id":{"type":"string"},"password":{"type":"string","format":"password"},"encryption":{"type":"string"},"flow":{"type":"string"},"servers":{"type":"array","items":{"type":"object"}},"vnext":{"type":"array","items":{"type":"object"}}}),&[]);
            }
            object(properties,&["tag","protocol"])
        }).collect();
        schemas.insert(name.into(),json!({"oneOf":alternatives,"discriminator":{"propertyName":"protocol"},"description":"Native Xray JSON; protocol-specific extensions are preserved and validated by the selected Xray version"}));
    }
    schemas.insert("RoutingRule".into(),object(json!({
        "ruleTag":{"type":"string"},"outboundTag":{"type":"string"},"balancerTag":{"type":"string"},
        "domain":{"type":"array","items":{"type":"string"}},"ip":{"type":"array","items":{"type":"string"}},
        "port":{"oneOf":[{"type":"string"},{"type":"integer"}]},"network":{"oneOf":[{"type":"string"},{"type":"array","items":{"type":"string"}}]},
        "inboundTag":{"type":"array","items":{"type":"string"}},"user":{"type":"array","items":{"type":"string"}},"protocol":{"type":"array","items":{"type":"string"}},
        "source":{"type":"array","items":{"type":"string"}},"sourcePort":{"type":"string"},"localIP":{"type":"array","items":{"type":"string"}},"localPort":{"type":"string"},"attrs":{"type":"object","additionalProperties":{"type":"string"}},"webhook":{"type":"object"}
    }),&[]));
    schemas.insert("BalancerConfig".into(),object(json!({"tag":{"type":"string"},"selector":{"type":"array","items":{"type":"string"}},"fallbackTag":{"type":"string"},"strategy":{"type":"object","properties":{"type":{"type":"string","enum":["random","roundRobin","leastPing","leastLoad"]},"settings":{"type":"object"}}}}),&["tag","selector"]));
    schemas.insert("RoutingConfig".into(),object(json!({"rules":{"type":"array","items":reference("RoutingRule")},"balancers":{"type":"array","items":reference("BalancerConfig")},"domainStrategy":{"type":"string"},"script":{"type":"string"}}),&[]));
    schemas.insert("RulesRequest".into(),json!({"type":"object","required":["routing"],"additionalProperties":false,"properties":{"routing":{"type":"object","additionalProperties":false,"properties":{"rules":{"type":"array","items":reference("RoutingRule")},"balancers":{"type":"array","items":reference("BalancerConfig")}}},"shouldAppend":{"type":"boolean","default":false}},"example":{"routing":{"rules":[{"ruleTag":"example","domain":["domain:example.com"],"outboundTag":"direct"}]},"shouldAppend":true}}));
    schemas.insert("XrayConfig".into(),object(json!({
        "inbounds":{"type":"array","items":reference("InboundConfig")},"outbounds":{"type":"array","items":reference("OutboundConfig")},"routing":reference("RoutingConfig"),
        "api":{"type":"object","properties":{"tag":{"type":"string"},"listen":{"type":"string","example":"127.0.0.1:10085"},"services":{"type":"array","items":{"type":"string"}}},"required":["tag","listen","services"]},
        "stats":{"type":"object"},"policy":{"type":"object"},"log":{"type":"object"},"dns":{"type":"object"},"observatory":{"type":"object"},"burstObservatory":{"type":"object"},"metrics":{"type":"object"},"fakeDns":{"type":"array","items":{"type":"object"}},"geodata":{"type":"object"},"env":{"type":"object","additionalProperties":{"type":"string"}}
    }),&["api","stats","inbounds","outbounds"]));
    schemas.insert("OutboundOperation".into(),json!({"type":"object","description":"Reserved for upstream AlterOutbound operations. v26.10.21 has no concrete operation and always returns 501."}));
    schemas.insert("InstallResult".into(),object(json!({"version":{"type":"string"},"already_installed":{"type":"boolean"},"cleanup_pending":{"type":"array","items":{"type":"object","properties":{"version":{"type":"string"},"error":{"type":"string"}}}}}),&["version","already_installed","cleanup_pending"]));
    schemas.insert("ControlResult".into(),object(json!({"revision":{"type":"integer","format":"int64"},"state":{"type":"string"},"stopped":{"type":"boolean"}}),&[]));
    schemas.insert(
        "DeleteResult".into(),
        object(json!({"deleted":{"type":"string"}}), &["deleted"]),
    );
    schemas.insert("Capabilities".into(),object(json!({"baseline":{"type":"string"},"active_version":{"type":["string","null"]},"rpcs":{"type":"array","items":{"type":"string"}},"configuration_format":{"type":"string"},"persistence":{"type":"string"},"limitations":{"type":"array","items":{"type":"string"}}}),&["baseline","rpcs","limitations"]));
    schemas.insert(
        "VersionList".into(),
        json!({"type":"array","items":reference("VersionInfo")}),
    );
    if let Some(snapshot) = schemas.get_mut("Snapshot") {
        snapshot["properties"]["config"] = reference("XrayConfig");
        snapshot["properties"]["overrides"] =
            json!({"type":"object","additionalProperties":{"type":"string"}});
    }
    // Replace generic user Value in derived operation schemas with the documented native shape.
    if let Some(operation) = schemas.get_mut("InboundOperation") {
        if let Some(variants) = operation.get_mut("oneOf").and_then(Value::as_array_mut) {
            for variant in variants {
                if variant.pointer("/properties/user").is_some() {
                    variant["properties"]["user"] = reference("NativeUser");
                }
            }
        }
    }
}

pub fn document() -> utoipa::openapi::OpenApi {
    // 先复用 utoipa 派生的业务模型，再补充动态 protobuf 类型和路由操作。
    let mut document =
        serde_json::to_value(ApiComponents::openapi()).expect("serialize OpenAPI components");
    document["info"] = json!({"title":"Rust Xray Agent","version":env!("CARGO_PKG_VERSION"),"description":"Root Linux/systemd agent. Configuration writes persist to SQLite and native Xray JSON without restarting. Full configuration replacement and explicit process controls may restart Xray. OpenAPI and Swagger UI contain no credentials."});
    document["components"]["securitySchemes"] =
        json!({"bearerAuth":{"type":"http","scheme":"bearer","bearerFormat":"opaque token"}});
    let schemas = document["components"]["schemas"]
        .as_object_mut()
        .expect("schema map");
    for message in grpc::pool()
        .all_messages()
        .filter(|message| !message.is_map_entry())
    {
        schemas.insert(
            protobuf_name(message.full_name()),
            protobuf_schema(&message),
        );
    }
    // TypedMessage.value 在接口中是展开对象；未知上游类型仍允许保留原始 Base64。
    let mut typed_variants:Vec<_>=grpc::pool().all_messages().filter(|message|!message.is_map_entry() && message.full_name()!="xray.common.serial.TypedMessage").map(|message|json!({"type":"object","required":["type","value"],"properties":{"type":{"type":"string","enum":[message.full_name()]},"value":reference(&protobuf_name(message.full_name()))}})).collect();
    typed_variants.push(json!({"type":"object","required":["type","value"],"properties":{"type":{"type":"string"},"value":{"type":"string","format":"byte"}},"description":"Unknown upstream TypedMessage types retain their Base64 payload"}));
    schemas.insert(protobuf_name("xray.common.serial.TypedMessage"),json!({"oneOf":typed_variants,"discriminator":{"propertyName":"type"},"description":"TypedMessage.value is expanded into the named protobuf message instead of Base64. Unknown upstream response types retain a Base64 value."}));
    native_schemas(schemas);
    let mut paths = Map::<String, Value>::new();
    for endpoint in ENDPOINTS {
        let mut parameters = Vec::new();
        for part in endpoint.path.split('/') {
            if let Some(name) = part
                .strip_prefix('{')
                .and_then(|part| part.strip_suffix('}'))
            {
                parameters.push(
                    json!({"name":name,"in":"path","required":true,"schema":{"type":"string"}}),
                );
            }
        }
        if endpoint.verb == "get" {
            // GET 参数来自 RPC 输入字段；已经绑定到路径的参数不再重复作为 query。
            if let Some(name) = endpoint.rpc {
                let method = grpc::method(name).unwrap();
                for field in method.input().fields() {
                    let json_name = field.json_name();
                    if endpoint.path.contains(&format!("{{{json_name}}}")) {
                        continue;
                    }
                    if name == "RoutingService/SubscribeRoutingStats" {
                        parameters.push(json!({"name":"fieldSelectors","in":"query","required":false,"style":"form","explode":false,"schema":{"type":"array","items":{"type":"string"}}}));
                    } else {
                        parameters.push(json!({"name":json_name,"in":"query","required":name.starts_with("StatsService/") && json_name=="name","schema":field_schema(&field)}));
                    }
                }
            }
        }
        let mut operation = json!({"operationId":format!("{}_{}",endpoint.verb,endpoint.path.trim_start_matches('/').replace(['/','{','}'],"_")),"summary":endpoint.summary,"tags":[endpoint.path.split('/').nth(4).unwrap_or("agent")],"security":[{"bearerAuth":[]}],"parameters":parameters});
        let input = endpoint.request.map(reference).or_else(|| {
            if endpoint.verb != "get" && endpoint.kind == Kind::Rpc {
                endpoint.rpc.map(|name| {
                    reference(&protobuf_name(
                        grpc::method(name).unwrap().input().full_name(),
                    ))
                })
            } else {
                None
            }
        });
        if let Some(input) = input {
            operation["requestBody"] = json!({"required":endpoint.kind!=Kind::Install,"content":{"application/json":{"schema":input}}});
        }
        // 配置变更附带提交修订号；纯 RPC 查询直接返回上游响应结构。
        let response=endpoint.response.map(reference).or_else(|| endpoint.rpc.map(|name| {
            let schema=reference(&protobuf_name(grpc::method(name).unwrap().output().full_name()));
            if endpoint.kind!=Kind::Rpc && endpoint.kind!=Kind::Events && endpoint.kind!=Kind::PatchOutbound {object(json!({"revision":{"type":"integer","format":"int64"},"response":schema}),&["revision","response"])} else {schema}
        })).unwrap_or(json!({"type":"object"}));
        let content = if endpoint.kind == Kind::Events {
            json!({"text/event-stream":{"schema":{"type":"string","description":"SSE route events carry RoutingContext JSON; error events terminate the stream."},"example":"event: route\ndata: {\"OutboundTag\":\"direct\"}\n\n"}})
        } else {
            json!({"application/json":{"schema":response}})
        };
        let mut responses = json!({"200":{"description":"Success","content":content}});
        for (status, description) in [
            ("400", "Invalid request"),
            ("401", "Missing or invalid Bearer token"),
            ("404", "Resource not found"),
            ("409", "Concurrent operation or reconciliation required"),
            ("413", "Request exceeds 4 MiB"),
            ("422", "Configuration validation failed"),
            ("500", "Internal failure"),
            ("501", "Upstream method/operation is unsupported"),
            ("502", "Upstream gRPC failure"),
            ("503", "Unavailable or mutation outcome needs confirmation"),
            ("504", "Upstream timeout"),
        ] {
            responses[status] = json!({"description":description,"content":{"application/json":{"schema":reference("ErrorBody")}}});
        }
        if endpoint.kind == Kind::PatchOutbound {
            // 基准版本没有可执行的出站 Alter 操作，不能在文档中承诺成功响应。
            responses.as_object_mut().unwrap().remove("200");
        }
        operation["responses"] = responses;
        paths.entry(endpoint.path).or_insert_with(|| json!({}))[endpoint.verb] = operation;
    }
    document["paths"] = Value::Object(paths);
    serde_json::from_value(document).expect("valid generated OpenAPI document")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn business_components_support_json_roundtrip() {
        let document = serde_json::to_value(ApiComponents::openapi()).unwrap();
        // 分别检查组件，使无效嵌套 schema 的报错包含组件名称，便于定位 RefOr 解析失败。
        for (name, schema) in document["components"]["schemas"].as_object().unwrap() {
            serde_json::from_value::<utoipa::openapi::RefOr<utoipa::openapi::schema::Schema>>(
                schema.clone(),
            )
            .unwrap_or_else(|error| panic!("invalid component {name}: {error}"));
        }
        for name in ["Snapshot", "Operation"] {
            let properties = &document["components"]["schemas"][name]["properties"];
            let fields: &[&str] = if name == "Snapshot" {
                &["config", "overrides"]
            } else {
                &["details"]
            };
            for field in fields {
                assert_eq!(properties[*field]["type"], "object", "{name}.{field}");
            }
        }
    }

    #[test]
    fn generated_document_supports_roundtrip_and_preserves_composite_schemas() {
        let value = serde_json::to_value(document()).unwrap();
        let parsed: utoipa::openapi::OpenApi = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), value);
        let schemas = &value["components"]["schemas"];
        assert_eq!(
            schemas["Operation"]["properties"]["details"]["type"],
            "object"
        );
        assert_eq!(
            schemas["Snapshot"]["properties"]["config"]["$ref"],
            "#/components/schemas/XrayConfig"
        );
        for name in [
            "InboundConfig",
            "OutboundConfig",
            "pb.xray.common.serial.TypedMessage",
        ] {
            assert!(
                schemas[name]["oneOf"].as_array().unwrap().len() > 1,
                "{name}"
            );
        }
    }
}
