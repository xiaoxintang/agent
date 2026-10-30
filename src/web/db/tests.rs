use super::{Agent, Assignment, Db};
use crate::web::Effect;
use serde_json::json;
use std::path::Path;

fn setup(path: &Path) -> (Db, Assignment, String) {
    let db = Db::open(path).unwrap();
    db.save_agent(&Agent {
        id: "node".into(),
        name: "test".into(),
        url: "http://127.0.0.1:8080".into(),
        token: "x".repeat(32),
    })
    .unwrap();
    let user = db.add_user("Xray user").unwrap();
    let a = Assignment {
        id: "binding".into(),
        user_id: user["id"].as_str().unwrap().into(),
        agent_id: "node".into(),
        inbound_tag: "vless".into(),
        native_user: json!({"email":"web-binding","id":"uuid"}),
        desired_present: true,
        generation: 1,
        status: "pending".into(),
    };
    let task = db.create_assignment(&a).unwrap();
    (db, a, task)
}

#[test]
fn revoke_supersedes_queued_grant_and_deletion_waits_for_acknowledgment() {
    let dir = tempfile::tempdir().unwrap();
    let (db, a, _) = setup(&dir.path().join("web.db"));
    let grant = db.due("node").unwrap().unwrap();
    db.delete_user(&a.user_id).unwrap();
    assert!(!db.current(&grant).unwrap());
    assert_eq!(db.users().unwrap()[0]["status"], "deleting");
    db.mark(&grant, "canceled", None, None, false).unwrap();
    let revoke = db.due("node").unwrap().unwrap();
    assert_eq!(revoke.request.method, "DELETE");
    db.mark(&revoke, "complete", None, None, false).unwrap();
    assert_eq!(db.assignments().unwrap()[0].status, "revoked");
    assert_eq!(db.users().unwrap()[0]["status"], "deleted");
}

#[test]
fn interrupted_write_is_uncertain_after_reopening() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("web.db");
    let (db, _, _) = setup(&path);
    let task = db.due("node").unwrap().unwrap();
    db.mark(&task, "running", None, None, true).unwrap();
    drop(db);
    let db = Db::open(&path).unwrap();
    db.conn()
        .unwrap()
        .execute("UPDATE tasks SET next_attempt=0", [])
        .unwrap();
    assert!(db.due("node").unwrap().unwrap().uncertain);
}

#[test]
fn stale_preview_rolls_back_all_effects_and_task_insert() {
    let dir = tempfile::tempdir().unwrap();
    let (db, a, _) = setup(&dir.path().join("web.db"));
    let before = db.tasks().unwrap().len();
    let e = Effect {
        id: a.id.clone(),
        generation: 99,
        present: false,
        native_user: a.native_user.clone(),
    };
    assert!(
        db.enqueue("node", &Db::assignment_request(&a), &[e])
            .is_err()
    );
    assert_eq!(db.tasks().unwrap().len(), before);
    assert_eq!(db.assignments().unwrap()[0].generation, 1);
}

#[test]
fn confirmed_revocation_can_be_assigned_again_with_new_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("db")).unwrap();
    db.local("http://127.0.0.1:8080").unwrap();
    let user = db.add_user("user").unwrap();
    let mut a = Assignment {
        id: "assignment".into(),
        user_id: user["id"].as_str().unwrap().into(),
        agent_id: "local".into(),
        inbound_tag: "tag".into(),
        native_user: json!({"email":"web-assignment","id":"old"}),
        desired_present: true,
        generation: 1,
        status: "pending".into(),
    };
    db.create_assignment(&a).unwrap();
    let grant = db.due("local").unwrap().unwrap();
    db.mark(&grant, "complete", None, None, false).unwrap();
    db.revoke(&a.id).unwrap();
    let revoke = db.due("local").unwrap().unwrap();
    db.mark(&revoke, "complete", None, None, false).unwrap();
    a.generation = 3;
    a.native_user["id"] = json!("new");
    db.create_assignment(&a).unwrap();
    let rows = db.assignments().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].generation, 3);
    assert_eq!(rows[0].native_user["id"], "new");
}
