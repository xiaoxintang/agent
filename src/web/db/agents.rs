use super::*;
use rusqlite::params;

impl Db {
    pub fn agents(&self) -> Result<Vec<Agent>> {
        let c = self.conn()?;
        let mut s = c.prepare("SELECT id,name,url,token FROM agents ORDER BY name")?;
        Ok(s.query_map([], |r| {
            Ok(Agent {
                id: r.get(0)?,
                name: r.get(1)?,
                url: r.get(2)?,
                token: r.get(3)?,
            })
        })?
        .collect::<std::result::Result<_, _>>()?)
    }

    pub fn agent(&self, id: &str) -> Result<Agent> {
        self.agents()?
            .into_iter()
            .find(|a| a.id == id)
            .ok_or_else(|| Error::NotFound("agent not found".into()))
    }

    pub fn save_agent(&self, a: &Agent) -> Result<()> {
        if a.id == "local" {
            return Err(Error::Validation(
                "the local agent is maintained by the installer".into(),
            ));
        }
        self.conn()?.execute("INSERT INTO agents VALUES(?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET name=excluded.name,url=excluded.url,token=excluded.token",params![a.id,a.name,a.url,a.token])?;
        Ok(())
    }

    pub fn local(&self, url: &str) -> Result<()> {
        self.conn()?.execute("INSERT INTO agents VALUES('local','本机',?1,'') ON CONFLICT(id) DO UPDATE SET url=excluded.url",[url])?;
        Ok(())
    }

    pub fn remove_agent(&self, id: &str) -> Result<()> {
        if id == "local" {
            return Err(Error::Validation("cannot remove the local agent".into()));
        }
        let c = self.conn()?;
        let count: i64 = c.query_row(
            "SELECT count(*) FROM assignments WHERE agent_id=?1 AND status!='revoked'",
            [id],
            |r| r.get(0),
        )?;
        let jobs:i64=c.query_row("SELECT count(*) FROM tasks WHERE agent_id=?1 AND status IN ('queued','retry','running')",[id],|r|r.get(0))?;
        if count > 0 || jobs > 0 {
            return Err(Error::Conflict(
                "revoke assignments and resolve tasks before removing this agent".into(),
            ));
        }
        let tx = c.unchecked_transaction()?;
        tx.execute("DELETE FROM assignments WHERE agent_id=?1", [id])?;
        tx.execute("DELETE FROM tasks WHERE agent_id=?1", [id])?;
        tx.execute("DELETE FROM agents WHERE id=?1", [id])?;
        tx.commit()?;
        Ok(())
    }
}
