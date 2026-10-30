use super::Invoke;
use crate::{
    api::ENDPOINTS,
    error::{Error, Result},
};

pub fn endpoint(r: &Invoke) -> Result<&'static crate::api::Endpoint> {
    let path = r.path.split('/').collect::<Vec<_>>();
    if r.path.contains(['?', '#', '\\'])
        || path
            .iter()
            .any(|s| matches!(*s, "." | "..") || s.to_lowercase().contains("%2e"))
    {
        return Err(Error::Validation("invalid agent path".into()));
    }
    ENDPOINTS
        .iter()
        .find(|e| {
            e.verb.eq_ignore_ascii_case(&r.method) && {
                let parts = e.path.split('/').collect::<Vec<_>>();
                parts.len() == path.len()
                    && parts.iter().zip(&path).all(|(a, b)| {
                        if a.starts_with('{') {
                            !b.is_empty()
                        } else {
                            a == b
                        }
                    })
            }
        })
        .ok_or_else(|| Error::Validation("operation is not registered in the Agent API".into()))
}

pub fn mutating(r: &Invoke) -> Result<bool> {
    let endpoint = endpoint(r)?;
    Ok(match endpoint.rpc {
        Some(
            "StatsService/GetStats" | "StatsService/QueryStats" | "StatsService/GetUsersStats",
        ) => r.body["reset"] == true,
        Some("RoutingService/TestRoute") => {
            r.body["PublishResult"] == true
                || r.body["publishResult"] == true
                || r.body["publish_result"] == true
        }
        _ => endpoint.mutating,
    })
}
