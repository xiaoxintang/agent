use super::{
    Invoke, Web,
    auth::{cookie, hash},
};
use crate::{
    error::{Error, Result},
    store::now,
    web_config::Credentials,
};
use axum::{
    body::Body,
    extract::{Path, Query, Request, State},
    http::header,
    response::Response,
};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc, time::Duration};

pub(super) async fn events(
    State(web): State<Arc<Web>>,
    Path(id): Path<String>,
    Query(query): Query<BTreeMap<String, String>>,
    request: Request,
) -> Result<Response> {
    let token = cookie(&request).unwrap_or_default();
    let epoch = Credentials::load(&web.credentials)?.epoch;
    let r = Invoke {
        method: "GET".into(),
        path: "/api/v1/xray/routing/events".into(),
        query,
        body: Value::Null,
        confirmation: None,
        expected_revision: None,
    };
    let a = web.agent(&id)?;
    let response = tokio::time::timeout(
        Duration::from_secs(20),
        web.client
            .get(format!("{}/api/v1/xray/routing/events", a.url))
            .bearer_auth(a.token)
            .query(&r.query)
            .send(),
    )
    .await
    .map_err(|_| Error::Unavailable("agent event headers timed out".into()))?
    .map_err(|_| Error::Unavailable("agent event connection failed".into()))?;
    let status = response.status();
    let content_type = response.headers().get(header::CONTENT_TYPE).cloned();
    let mut source = Box::pin(response.bytes_stream());
    use futures_util::StreamExt;
    let stream = async_stream::stream! {
        let mut interval=tokio::time::interval(Duration::from_secs(2));
        loop {tokio::select!{
            item=source.next()=>match item{Some(value)=>yield value,None=>break},
            _=interval.tick()=>{
                let valid=Credentials::load(&web.credentials).is_ok_and(|c|c.epoch==epoch)&&web.db.conn().ok().is_some_and(|c|c.query_row("SELECT EXISTS(SELECT 1 FROM sessions WHERE token_hash=?1 AND expires>?2)",rusqlite::params![hash(&token),now()],|r|r.get::<_,bool>(0)).unwrap_or(false));
                if !valid {break;}
            }
        }}
    };
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = status;
    if let Some(ct) = content_type {
        response.headers_mut().insert(header::CONTENT_TYPE, ct);
    }
    Ok(response)
}
