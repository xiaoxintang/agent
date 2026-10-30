use super::*;
use crate::store::now;
use crate::web::{Effect, Invoke};
use rusqlite::params;

impl Db {
    pub(super) fn insert_task(
        c: &Connection,
        agent: &str,
        request: &Invoke,
        effects: &[Effect],
    ) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        c.execute("INSERT INTO tasks(id,agent_id,request,effects,status,created_at) VALUES(?1,?2,?3,?4,'queued',?5)",params![id,agent,serde_json::to_string(request)?,serde_json::to_string(effects)?,now()])?;
        Ok(id)
    }

    pub(super) fn apply_effects(c: &Connection, effects: &[Effect]) -> Result<()> {
        for e in effects {
            if e.present {
                let active:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM assignments a JOIN users u ON u.id=a.user_id WHERE a.id=?1 AND u.status='active')",[&e.id],|r|r.get(0))?;
                if !active {
                    return Err(Error::Conflict(
                        "cannot grant access to a deleting or deleted global user".into(),
                    ));
                }
            }
            let changed=c.execute("UPDATE assignments SET native_user=?2,desired_present=?3,generation=?4,status='pending' WHERE id=?1 AND generation=?5",params![e.id,e.native_user.to_string(),e.present,e.generation,e.generation-1])?;
            if changed != 1 {
                return Err(Error::Conflict("assignment changed; preview again".into()));
            }
        }
        Ok(())
    }

    pub fn enqueue(&self, agent: &str, request: &Invoke, effects: &[Effect]) -> Result<String> {
        let c = self.conn()?;
        let tx = c.unchecked_transaction()?;
        Self::apply_effects(&tx, effects)?;
        let id = Self::insert_task(&tx, agent, request, effects)?;
        tx.commit()?;
        Ok(id)
    }

    pub fn repair_pending(&self) -> Result<()> {
        let assignments = self.assignments()?;
        let c = self.conn()?;
        let tx = c.unchecked_transaction()?;
        for a in assignments.into_iter().filter(|a| a.status == "pending") {
            let exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM tasks t,json_each(t.effects) e WHERE t.status IN ('queued','retry','running','blocked') AND json_extract(e.value,'$.id')=?1 AND json_extract(e.value,'$.generation')=?2)",params![a.id,a.generation],|r|r.get(0))?;
            if !exists {
                Self::insert_task(
                    &tx,
                    &a.agent_id,
                    &Self::assignment_request(&a),
                    &[Effect::from(&a)],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }
}
