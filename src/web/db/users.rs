use super::*;
use crate::web::Effect;
use rusqlite::params;
use serde_json::json;

impl Db {
    pub fn users(&self) -> Result<Vec<Value>> {
        let c = self.conn()?;
        let mut s = c.prepare("SELECT id,name,status FROM users ORDER BY name")?;
        Ok(s.query_map([],|r|Ok(json!({"id":r.get::<_,String>(0)?,"name":r.get::<_,String>(1)?,"status":r.get::<_,String>(2)?})))?.collect::<std::result::Result<_,_>>()?)
    }

    pub fn add_user(&self, name: &str) -> Result<Value> {
        if name.trim().is_empty() {
            return Err(Error::Validation("name is required".into()));
        }
        let id = uuid::Uuid::new_v4().to_string();
        self.conn()?.execute(
            "INSERT INTO users(id,name) VALUES(?1,?2)",
            params![id, name],
        )?;
        Ok(json!({"id":id,"name":name,"status":"active"}))
    }

    pub fn delete_user(&self, id: &str) -> Result<()> {
        let c = self.conn()?;
        let tx = c.unchecked_transaction()?;
        let assignments = Self::assignments_from(&tx)?
            .into_iter()
            .filter(|a| a.user_id == id && a.status != "revoked")
            .collect::<Vec<_>>();
        if tx.execute(
            "UPDATE users SET status='deleting' WHERE id=?1 AND status!='deleted'",
            [id],
        )? == 0
        {
            return Err(Error::NotFound("Xray user not found".into()));
        }
        for mut a in assignments {
            a.desired_present = false;
            a.generation += 1;
            let e = Effect::from(&a);
            Self::apply_effects(&tx, &[e.clone()])?;
            Self::insert_task(&tx, &a.agent_id, &Self::assignment_request(&a), &[e])?;
        }
        tx.execute("UPDATE users SET status='deleted' WHERE id=?1 AND NOT EXISTS(SELECT 1 FROM assignments WHERE user_id=?1 AND status!='revoked')",[id])?;
        tx.commit()?;
        Ok(())
    }
}
