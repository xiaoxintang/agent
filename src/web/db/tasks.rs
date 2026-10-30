use super::*;
use crate::store::now;
use crate::web::Task;
use rusqlite::OptionalExtension;
use rusqlite::params;
use serde_json::json;

impl Db {
    pub fn tasks(&self) -> Result<Vec<Value>> {
        let c = self.conn()?;
        let mut s=c.prepare("SELECT id,agent_id,request,status,attempts,error,result,created_at FROM tasks WHERE status NOT IN ('complete','canceled') OR id IN (SELECT id FROM tasks WHERE status IN ('complete','canceled') ORDER BY created_at DESC,rowid DESC LIMIT 200) ORDER BY created_at DESC,rowid DESC")?;
        let rows = s
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, i64>(7)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows.into_iter().map(|(id,agent_id,request,status,attempts,error,result,created_at)|Ok(json!({"id":id,"agent_id":agent_id,"request":serde_json::from_str::<Value>(&request)?,"status":status,"attempts":attempts,"error":error,"result":result.and_then(|s|serde_json::from_str::<Value>(&s).ok()),"created_at":created_at}))).collect()
    }

    pub fn due(&self, agent: &str) -> Result<Option<Task>> {
        let c = self.conn()?;
        let row=c.query_row("SELECT id,request,effects,attempts,uncertain FROM tasks WHERE agent_id=?1 AND status IN ('queued','retry') AND next_attempt<=?2 ORDER BY created_at,rowid LIMIT 1",params![agent,now()],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,u32>(3)?,r.get::<_,bool>(4)?))).optional()?;
        row.map(|(id, r, e, attempts, uncertain)| {
            Ok(Task {
                id,
                agent_id: agent.into(),
                request: serde_json::from_str(&r)?,
                effects: serde_json::from_str(&e)?,
                attempts,
                uncertain,
            })
        })
        .transpose()
    }

    pub fn current(&self, task: &Task) -> Result<bool> {
        let c = self.conn()?;
        for e in &task.effects {
            let matches: bool = c.query_row(
                "SELECT EXISTS(SELECT 1 FROM assignments WHERE id=?1 AND generation=?2)",
                params![e.id, e.generation],
                |r| r.get(0),
            )?;
            if !matches {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub fn mark(
        &self,
        task: &Task,
        status: &str,
        error: Option<&str>,
        result: Option<&Value>,
        uncertain: bool,
    ) -> Result<()> {
        let c = self.conn()?;
        let tx = c.unchecked_transaction()?;
        let delay = std::cmp::min(300, 2_i64.pow(std::cmp::min(task.attempts + 1, 8)));
        tx.execute("UPDATE tasks SET status=?2,error=?3,result=?4,attempts=attempts+1,next_attempt=?5,uncertain=?6 WHERE id=?1",params![task.id,status,error,result.map(Value::to_string),now()+delay,uncertain])?;
        if status == "complete" {
            for e in &task.effects {
                tx.execute(
                    "UPDATE assignments SET status=?3 WHERE id=?1 AND generation=?2",
                    params![
                        e.id,
                        e.generation,
                        if e.present { "applied" } else { "revoked" }
                    ],
                )?;
            }
            tx.execute("UPDATE users SET status='deleted' WHERE status='deleting' AND NOT EXISTS(SELECT 1 FROM assignments WHERE assignments.user_id=users.id AND status!='revoked')",[])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn retry(&self, id: &str) -> Result<()> {
        let n=self.conn()?.execute("UPDATE tasks SET status='retry',next_attempt=0 WHERE id=?1 AND status IN ('blocked','retry')",[id])?;
        if n == 0 {
            return Err(Error::Conflict("task cannot be retried".into()));
        }
        Ok(())
    }
}
