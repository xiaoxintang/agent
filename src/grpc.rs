//! 依据固定 proto 描述集动态编解码管理 RPC，并展开 Xray 的 TypedMessage。
//! 同一描述集同时支撑请求校验、能力清单和 OpenAPI，避免维护多份字段定义。

use crate::error::{Error, Result};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::{Stream, StreamExt};
use prost::Message;
use prost_reflect::{DescriptorPool, DynamicMessage, Kind, MessageDescriptor, MethodDescriptor};
use serde_json::Value;
use std::{pin::Pin, sync::OnceLock, time::Duration};
use tonic::{
    codec::{Codec, DecodeBuf, Decoder, EncodeBuf, Encoder},
    transport::{Channel, Endpoint},
};

// 只开放管理服务；代理数据传输使用的 gRPC 服务不属于这个调用入口。
pub const SERVICES: &[(&str, &str)] = &[
    ("HandlerService", "xray.app.proxyman.command.HandlerService"),
    ("StatsService", "xray.app.stats.command.StatsService"),
    ("RoutingService", "xray.app.router.command.RoutingService"),
    ("LoggerService", "xray.app.log.command.LoggerService"),
    (
        "ObservatoryService",
        "xray.core.app.observatory.command.ObservatoryService",
    ),
];

pub fn pool() -> &'static DescriptorPool {
    // 描述文件在构建时生成并嵌入二进制，运行时不依赖 proto 文件或外网。
    static POOL: OnceLock<DescriptorPool> = OnceLock::new();
    POOL.get_or_init(|| {
        DescriptorPool::decode(include_bytes!(concat!(env!("OUT_DIR"), "/xray.bin")).as_ref())
            .expect("valid pinned descriptors")
    })
}

pub fn method(name: &str) -> Result<MethodDescriptor> {
    // 先按白名单解析短服务名，不能让 HTTP 调用方构造任意 gRPC 方法路径。
    let (service, name) = name
        .split_once('/')
        .ok_or_else(|| Error::BadRequest("expected Service/Method".into()))?;
    let full_name = SERVICES
        .iter()
        .find(|(short, _)| *short == service)
        .map(|(_, full)| *full)
        .ok_or_else(|| Error::Unsupported("unknown management service".into()))?;
    pool()
        .get_service_by_name(full_name)
        .and_then(|service| service.methods().find(|method| method.name() == name))
        .ok_or_else(|| Error::Unsupported("unknown management method".into()))
}

pub fn methods() -> Vec<String> {
    SERVICES
        .iter()
        .flat_map(|(short, full)| {
            pool()
                .get_service_by_name(full)
                .unwrap()
                .methods()
                .map(|method| format!("{short}/{}", method.name()))
                .collect::<Vec<_>>()
        })
        .collect()
}

fn transform(descriptor: &MessageDescriptor, value: &mut Value, pack: bool) -> Result<()> {
    // pack=true：把结构化 value 编码为 protobuf bytes；false：把 bytes 展开为对象。
    // 普通字段按 descriptor 递归，保证嵌套账户、列表和 map 中的 TypedMessage 同样生效。
    if descriptor.full_name() == "xray.common.serial.TypedMessage" {
        let type_name = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let Some(target) = pool().get_message_by_name(&type_name) else {
            if pack {
                return Err(Error::BadRequest(format!(
                    "unknown TypedMessage type: {type_name}"
                )));
            }
            // 新版上游返回未知类型时保留原始 Base64；不能猜测其字段结构。
            return Ok(());
        };
        if !pack && value.get("value").is_none() {
            // Protobuf JSON 会省略默认空 bytes，但空消息仍应展开成空对象。
            value["value"] = Value::String(String::new());
        }
        if let Some(inner) = value.get_mut("value") {
            if pack && inner.is_object() {
                let message = from_json(target, inner.clone())?;
                *inner = Value::String(STANDARD.encode(message.encode_to_vec()));
            } else if !pack && inner.is_string() {
                let bytes = STANDARD
                    .decode(inner.as_str().unwrap())
                    .map_err(|error| Error::Internal(error.to_string()))?;
                let message = DynamicMessage::decode(target, bytes.as_slice())
                    .map_err(|error| Error::Internal(error.to_string()))?;
                *inner = to_json(&message)?;
            }
        }
        return Ok(());
    }
    if let Some(object) = value.as_object_mut() {
        for (name, value) in object {
            let Some(field) = descriptor
                .get_field_by_json_name(name)
                .or_else(|| descriptor.get_field_by_name(name))
            else {
                continue;
            };
            let Kind::Message(child) = field.kind() else {
                continue;
            };
            if field.is_map() {
                // protobuf map 实际是合成 entry 消息，只递归其 value 的消息类型。
                if let Kind::Message(map_value) = child.get_field_by_name("value").unwrap().kind() {
                    if let Some(map) = value.as_object_mut() {
                        for value in map.values_mut() {
                            transform(&map_value, value, pack)?;
                        }
                    }
                }
            } else if field.is_list() {
                if let Some(list) = value.as_array_mut() {
                    for value in list {
                        transform(&child, value, pack)?;
                    }
                }
            } else {
                transform(&child, value, pack)?;
            }
        }
    }
    Ok(())
}

pub fn from_json(descriptor: MessageDescriptor, mut value: Value) -> Result<DynamicMessage> {
    // 先处理 Xray 自定义的包装，再由标准 Protobuf JSON 解析器校验字段和值。
    transform(&descriptor, &mut value, true)?;
    let bytes = serde_json::to_vec(&value)?;
    let mut deserializer = serde_json::Deserializer::from_slice(&bytes);
    DynamicMessage::deserialize(descriptor, &mut deserializer)
        .map_err(|error| Error::BadRequest(format!("invalid protobuf JSON: {error}")))
}

pub fn to_json(message: &DynamicMessage) -> Result<Value> {
    use prost_reflect::ReflectMessage;
    // 标准序列化保留 int64 的字符串表示，避免调用方丢失大整数精度。
    let mut value = serde_json::to_value(message)?;
    transform(&message.descriptor(), &mut value, false)?;
    Ok(value)
}

pub type EventStream = Pin<Box<dyn Stream<Item = Result<Value>> + Send>>;

#[async_trait]
pub trait XrayRpc: Send + Sync {
    async fn unary(&self, name: &str, request: Value) -> Result<Value>;
    async fn subscribe(&self, request: Value) -> Result<EventStream>;
}

pub struct Grpc {
    channel: Channel,
}

impl Grpc {
    pub fn new(address: std::net::SocketAddr) -> Result<Self> {
        let endpoint = Endpoint::from_shared(format!("http://{address}"))
            .map_err(|error| Error::Validation(error.to_string()))?
            .connect_timeout(Duration::from_secs(3));
        Ok(Self {
            // Agent 可以在 Xray 尚未安装或启动时运行，首次 RPC 再建立连接。
            channel: endpoint.connect_lazy(),
        })
    }
}

#[async_trait]
impl XrayRpc for Grpc {
    async fn unary(&self, name: &str, request: Value) -> Result<Value> {
        let method = method(name)?;
        let message = from_json(method.input(), request)?;
        let mut request = tonic::Request::new(message);
        // 既向服务端传递 deadline，也在客户端限制等待；超时不证明变更未生效。
        request.set_timeout(Duration::from_secs(10));
        let mut client = tonic::client::Grpc::new(self.channel.clone());
        client
            .ready()
            .await
            .map_err(|error| Error::Unavailable(format!("gRPC channel unavailable: {error}")))?;
        let path = http::uri::PathAndQuery::from_maybe_shared(format!(
            "/{}/{}",
            method.parent_service().full_name(),
            method.name()
        ))
        .unwrap();
        let response = tokio::time::timeout(
            Duration::from_secs(10),
            client.unary(request, path, DynamicCodec(method.output())),
        )
        .await
        .map_err(|_| Error::Grpc {
            code: tonic::Code::DeadlineExceeded,
            message: "gRPC deadline exceeded; mutation outcome may be unknown".into(),
        })??;
        to_json(response.get_ref())
    }

    async fn subscribe(&self, request: Value) -> Result<EventStream> {
        let method = method("RoutingService/SubscribeRoutingStats")?;
        let message = from_json(method.input(), request)?;
        let mut client = tonic::client::Grpc::new(self.channel.clone());
        client
            .ready()
            .await
            .map_err(|error| Error::Unavailable(format!("gRPC channel unavailable: {error}")))?;
        let path = http::uri::PathAndQuery::from_static(
            "/xray.app.router.command.RoutingService/SubscribeRoutingStats",
        );
        // 这里只限制建立订阅的时间，不给已经建立的长期事件流设置十秒总时限。
        let response = tokio::time::timeout(
            Duration::from_secs(10),
            client.server_streaming(
                tonic::Request::new(message),
                path,
                DynamicCodec(method.output()),
            ),
        )
        .await
        .map_err(|_| Error::Unavailable("routing stream connection timed out".into()))??;
        Ok(Box::pin(response.into_inner().map(|result| {
            result
                .map_err(Error::from)
                .and_then(|message| to_json(&message))
        })))
    }
}

// 响应类型由方法描述符决定；编码继续使用 protobuf wire format，不是 JSON 透传。
struct DynamicCodec(MessageDescriptor);
struct DynamicEncoder;
struct DynamicDecoder(MessageDescriptor);

impl Codec for DynamicCodec {
    type Encode = DynamicMessage;
    type Decode = DynamicMessage;
    type Encoder = DynamicEncoder;
    type Decoder = DynamicDecoder;
    fn encoder(&mut self) -> Self::Encoder {
        DynamicEncoder
    }
    fn decoder(&mut self) -> Self::Decoder {
        DynamicDecoder(self.0.clone())
    }
}

impl Encoder for DynamicEncoder {
    type Item = DynamicMessage;
    type Error = tonic::Status;
    fn encode(
        &mut self,
        item: Self::Item,
        destination: &mut EncodeBuf<'_>,
    ) -> std::result::Result<(), Self::Error> {
        item.encode(destination)
            .map_err(|error| tonic::Status::internal(error.to_string()))
    }
}

impl Decoder for DynamicDecoder {
    type Item = DynamicMessage;
    type Error = tonic::Status;
    fn decode(
        &mut self,
        source: &mut DecodeBuf<'_>,
    ) -> std::result::Result<Option<Self::Item>, Self::Error> {
        DynamicMessage::decode(self.0.clone(), source)
            .map(Some)
            .map_err(|error| tonic::Status::internal(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn all_management_methods_and_nested_accounts_are_available() {
        assert_eq!(methods().len(), 26);
        let input = json!({"tag":"vless", "operation":{"type":"xray.app.proxyman.command.AddUserOperation","value":{"user":{"email":"a@example.com","account":{"type":"xray.proxy.vless.Account","value":{"id":"01234567-89ab-cdef-0123-456789abcdef"}}}}}});
        let message = from_json(
            method("HandlerService/AlterInbound").unwrap().input(),
            input.clone(),
        )
        .unwrap();
        assert_eq!(to_json(&message).unwrap(), input);
    }

    #[test]
    fn int64_stats_remain_strings() {
        let descriptor = method("StatsService/GetStats").unwrap().output();
        let input = json!({"stat":{"name":"large","value":"9007199254740993"}});
        assert_eq!(
            to_json(&from_json(descriptor, input.clone()).unwrap()).unwrap(),
            input
        );
    }
}
