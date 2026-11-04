use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub const STATS_PORT: u16 = 10085;
pub const TRAFFIC_SECONDS: u64 = 15;
pub const MAX_COUNTER: i64 = 9_007_199_254_740_991;

#[derive(Clone, Debug, Deserialize, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct ClientCounter {
    pub client_id: String,
    #[ts(type = "number")]
    pub uplink_bytes: i64,
    #[ts(type = "number")]
    pub downlink_bytes: i64,
}
#[derive(Clone, Debug, Deserialize, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct TrafficSample {
    pub instance_id: String,
    #[ts(type = "number")]
    pub collected_at: i64,
    pub counters: Vec<ClientCounter>,
}
impl TrafficSample {
    pub fn valid(&self, now: i64) -> bool {
        let mut seen = std::collections::BTreeSet::new();
        self.instance_id.len() == 32
            && self
                .instance_id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            && now.abs_diff(self.collected_at) <= 60
            && self.counters.len() <= 256
            && self.counters.iter().all(|counter| {
                uuid::Uuid::parse_str(&counter.client_id).is_ok()
                    && seen.insert(&counter.client_id)
                    && (0..=MAX_COUNTER).contains(&counter.uplink_bytes)
                    && (0..=MAX_COUNTER).contains(&counter.downlink_bytes)
            })
    }
}
#[derive(Clone, Debug, Deserialize, Serialize, TS)]
#[serde(tag = "status", content = "data", rename_all = "snake_case")]
pub enum TrafficReport {
    Available(TrafficSample),
    Unavailable,
}
#[derive(Deserialize, Serialize, TS)]
pub struct TrafficBucket {
    #[ts(type = "number")]
    pub bucket_start: i64,
    pub uplink_bytes: String,
    pub downlink_bytes: String,
}
#[derive(Deserialize, Serialize, TS)]
pub struct ClientTraffic {
    pub client_id: String,
    pub display_name: String,
    pub uplink_bytes: String,
    pub downlink_bytes: String,
}
#[derive(Deserialize, Serialize, TS)]
pub struct NodeTraffic {
    pub available: bool,
    pub stale: bool,
    #[ts(type = "number | null")]
    pub received_at: Option<i64>,
    pub buckets: Vec<TrafficBucket>,
    pub clients: Vec<ClientTraffic>,
}

/// First observation, process replacement, reset or an implausible jump creates a baseline.
pub fn delta(
    previous: Option<(&str, i64, i64, i64)>,
    sample: &TrafficSample,
    counter: &ClientCounter,
) -> (i64, i64) {
    let Some((instance, at, up, down)) = previous else {
        return (0, 0);
    };
    if instance != sample.instance_id
        || sample.collected_at <= at
        || counter.uplink_bytes < up
        || counter.downlink_bytes < down
    {
        return (0, 0);
    }
    let delta = (counter.uplink_bytes - up, counter.downlink_bytes - down);
    let limit = (sample.collected_at - at).saturating_mul(64 * 1024 * 1024 * 1024);
    if delta.0.saturating_add(delta.1) > limit {
        (0, 0)
    } else {
        delta
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn counters_never_charge_duplicates_resets_restarts_or_huge_jumps() {
        let counter = ClientCounter {
            client_id: uuid::Uuid::new_v4().to_string(),
            uplink_bytes: 120,
            downlink_bytes: 240,
        };
        let sample = TrafficSample {
            instance_id: "a".repeat(32),
            collected_at: 100,
            counters: vec![counter.clone()],
        };
        assert_eq!(delta(None, &sample, &counter), (0, 0));
        assert_eq!(
            delta(Some((&sample.instance_id, 85, 100, 200)), &sample, &counter),
            (20, 40)
        );
        for previous in [
            ("b".repeat(32), 85, 100, 200),
            (sample.instance_id.clone(), 100, 100, 200),
            (sample.instance_id.clone(), 85, 1000, 200),
        ] {
            assert_eq!(
                delta(
                    Some((&previous.0, previous.1, previous.2, previous.3)),
                    &sample,
                    &counter
                ),
                (0, 0)
            );
        }
        assert!(
            !TrafficSample {
                collected_at: i64::MIN,
                ..sample.clone()
            }
            .valid(100)
        );
        assert!(
            !TrafficSample {
                counters: vec![counter.clone(), counter.clone()],
                ..sample.clone()
            }
            .valid(100)
        );
        let huge = ClientCounter {
            uplink_bytes: MAX_COUNTER,
            ..counter
        };
        assert_eq!(
            delta(Some((&sample.instance_id, 85, 100, 200)), &sample, &huge),
            (0, 0)
        );
    }
}
