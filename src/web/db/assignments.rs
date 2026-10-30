use super::*;
use crate::web::{Effect, Invoke, segment};
use rusqlite::params;

impl Db {
    pub fn assignments(&self) -> Result<Vec<Assignment>> {
        let c = self.conn()?;
        Self::assignments_from(&c)
    }

    pub(super) fn assignments_from(c: &Connection) -> Result<Vec<Assignment>> {
        let mut s=c.prepare("SELECT id,user_id,agent_id,inbound_tag,native_user,desired_present,generation,status FROM assignments")?;
        let rows = s
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, bool>(5)?,
                    r.get::<_, i64>(6)?,
                    r.get::<_, String>(7)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(
                |(id, user_id, agent_id, inbound_tag, u, desired_present, generation, status)| {
                    Ok(Assignment {
                        id,
                        user_id,
                        agent_id,
                        inbound_tag,
                        native_user: serde_json::from_str(&u)?,
                        desired_present,
                        generation,
                        status,
                    })
                },
            )
            .collect()
    }

    pub fn create_assignment(&self, a: &Assignment) -> Result<String> {
        let c = self.conn()?;
        let tx = c.unchecked_transaction()?;
        let active: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM users WHERE id=?1 AND status='active')",
            [&a.user_id],
            |r| r.get(0),
        )?;
        if !active {
            return Err(Error::Validation(
                "global Xray user is missing or deleting".into(),
            ));
        }
        let identity = a
            .native_user
            .get("email")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let collision:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM assignments WHERE agent_id=?1 AND inbound_tag=?2 AND lower(json_extract(native_user,'$.email'))=lower(?3) AND id!=?4 AND status!='revoked')",params![a.agent_id,a.inbound_tag,identity,a.id],|r|r.get(0))?;
        if collision {
            return Err(Error::Conflict(
                "this inbound identity is already assigned".into(),
            ));
        }
        let changed=tx.execute("INSERT INTO assignments VALUES(?1,?2,?3,?4,?5,1,?6,'pending') ON CONFLICT(user_id,agent_id,inbound_tag) DO UPDATE SET native_user=excluded.native_user,desired_present=1,generation=excluded.generation,status='pending' WHERE assignments.status='revoked' AND assignments.id=excluded.id AND assignments.generation=excluded.generation-1",params![a.id,a.user_id,a.agent_id,a.inbound_tag,a.native_user.to_string(),a.generation])?;
        if changed != 1 {
            return Err(Error::Conflict(
                "assignment is active or changed; reload before assigning".into(),
            ));
        }
        // Adoption also goes through runtime reconciliation; configuration alone is not an acknowledgment.
        let id = Self::insert_task(
            &tx,
            &a.agent_id,
            &Self::assignment_request(a),
            &[Effect::from(a)],
        )?;
        tx.commit()?;
        Ok(id)
    }

    pub fn assignment_request(a: &Assignment) -> Invoke {
        let tag = segment(&a.inbound_tag);
        let (method, path, body) = if a.desired_present {
            (
                "POST",
                format!("/api/v1/xray/inbounds/{tag}/users"),
                a.native_user.clone(),
            )
        } else {
            (
                "DELETE",
                format!(
                    "/api/v1/xray/inbounds/{tag}/users/{}",
                    segment(a.native_user["email"].as_str().unwrap_or_default())
                ),
                Value::Null,
            )
        };
        Invoke {
            method: method.into(),
            path,
            body,
            query: Default::default(),
            confirmation: None,
            expected_revision: None,
        }
    }

    pub fn revoke(&self, id: &str) -> Result<String> {
        let mut a = self
            .assignments()?
            .into_iter()
            .find(|a| a.id == id)
            .ok_or_else(|| Error::NotFound("assignment not found".into()))?;
        a.desired_present = false;
        a.generation += 1;
        self.enqueue(
            &a.agent_id,
            &Self::assignment_request(&a),
            &[Effect::from(&a)],
        )
    }
}
