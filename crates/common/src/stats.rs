//! Wire-compatible subset of Xray v26.10.31 app/stats/command/command.proto.
//! Field tags and RPC path are kept beside the pinned upstream source in proto/.
use crate::traffic::{ClientCounter, MAX_COUNTER, STATS_PORT};
use std::{collections::BTreeMap, time::Duration};
use tonic::{client::Grpc, codegen::http::uri::PathAndQuery, transport::Channel};

#[derive(Clone, PartialEq, prost::Message)]
struct QueryStatsRequest {
    #[prost(string, tag = "1")]
    pattern: String,
    #[prost(bool, tag = "2")]
    reset: bool,
}
#[derive(Clone, PartialEq, prost::Message)]
struct Stat {
    #[prost(string, tag = "1")]
    name: String,
    #[prost(int64, tag = "2")]
    value: i64,
}
#[derive(Clone, PartialEq, prost::Message)]
struct QueryStatsResponse {
    #[prost(message, repeated, tag = "1")]
    stat: Vec<Stat>,
}

pub async fn counters() -> Result<Vec<ClientCounter>, &'static str> {
    let channel = Channel::from_shared(format!("http://127.0.0.1:{STATS_PORT}"))
        .map_err(|_| "stats_unavailable")?
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(2))
        .connect()
        .await
        .map_err(|_| "stats_unavailable")?;
    let mut client = Grpc::new(channel)
        .max_decoding_message_size(64 * 1024)
        .max_encoding_message_size(1024);
    client.ready().await.map_err(|_| "stats_unavailable")?;
    let response: tonic::Response<QueryStatsResponse> = client
        .unary(
            tonic::Request::new(QueryStatsRequest {
                pattern: "user>>>".into(),
                reset: false,
            }),
            PathAndQuery::from_static("/xray.app.stats.command.StatsService/QueryStats"),
            tonic_prost::ProstCodec::default(),
        )
        .await
        .map_err(|_| "stats_unavailable")?;
    let stats = response.into_inner().stat;
    if stats.len() > 512 {
        return Err("stats_limit");
    }
    let mut users = BTreeMap::<String, (Option<i64>, Option<i64>)>::new();
    for stat in stats {
        let parts: Vec<_> = stat.name.split(">>>").collect();
        if parts.len() != 4
            || parts[0] != "user"
            || parts[2] != "traffic"
            || uuid::Uuid::parse_str(parts[1]).is_err()
            || !(0..=MAX_COUNTER).contains(&stat.value)
        {
            continue;
        }
        let entry = users.entry(parts[1].into()).or_default();
        let slot = match parts[3] {
            "uplink" => &mut entry.0,
            "downlink" => &mut entry.1,
            _ => continue,
        };
        if slot.replace(stat.value).is_some() {
            return Err("duplicate_stat");
        }
    }
    Ok(users
        .into_iter()
        .map(|(client_id, (up, down))| ClientCounter {
            client_id,
            uplink_bytes: up.unwrap_or(0),
            downlink_bytes: down.unwrap_or(0),
        })
        .collect())
}
