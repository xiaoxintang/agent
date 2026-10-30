mod effects;
mod executor;
mod reconcile;
#[cfg(test)]
mod tests;

use super::Web;
pub(super) use effects::{configuration_write, effects, native_users};
use executor::step;
use std::{collections::HashSet, sync::Arc, time::Duration};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

pub async fn run(web: Arc<Web>, stop: CancellationToken) {
    let mut tasks = JoinSet::new();
    let mut active = HashSet::new();
    loop {
        tokio::select! {
            _=stop.cancelled()=>break,
            Some(result)=tasks.join_next(),if !tasks.is_empty()=>match result{Ok(id)=>{active.remove(&id);},Err(e)=>{tracing::error!(error=%e,"Web worker terminated");active.clear();}},
            _=tokio::time::sleep(Duration::from_secs(2))=>{
                if let Err(e)=web.db.repair_pending(){tracing::error!(error=%e,"cannot repair pending assignments");}
                match web.db.agents(){
                    Ok(agents)=>for agent in agents{if active.insert(agent.id.clone()){
                        let state=web.clone();tasks.spawn(async move{if let Err(e)=step(&state,&agent.id).await{tracing::error!(error=%e,agent=%agent.id,"Web task failed");}agent.id});
                    }},
                    Err(e)=>tracing::error!(error=%e,"cannot load agents"),
                }
            }
        }
    }
    while tasks.join_next().await.is_some() {}
}
