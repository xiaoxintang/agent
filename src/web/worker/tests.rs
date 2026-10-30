use super::{effects, reconcile::runtime_credentials_match};
use crate::web::{Invoke, db::Assignment};
use serde_json::{Value, json};

fn assignment() -> Assignment {
    Assignment {
        id: "a".into(),
        user_id: "u".into(),
        agent_id: "n".into(),
        inbound_tag: "入口/a".into(),
        native_user: json!({"email":"web-a","id":"secret"}),
        desired_present: true,
        generation: 2,
        status: "applied".into(),
    }
}

#[test]
fn native_deletion_advances_intent_and_preserves_credentials() {
    let a = assignment();
    let r = Invoke {
        method: "DELETE".into(),
        path: format!(
            "/api/v1/xray/inbounds/{}",
            crate::web::segment(&a.inbound_tag)
        ),
        query: Default::default(),
        body: Value::Null,
        confirmation: None,
        expected_revision: None,
    };
    let effects = effects(std::slice::from_ref(&a), "n", &r);
    assert_eq!(effects.len(), 1);
    assert!(!effects[0].present);
    assert_eq!(effects[0].generation, 3);
    assert_eq!(effects[0].native_user, a.native_user);
}

#[test]
fn config_replacement_compares_credentials_not_only_email() {
    let a = assignment();
    let r = Invoke {
        method: "PUT".into(),
        path: "/api/v1/xray/config".into(),
        query: Default::default(),
        body: json!({"inbounds":[{"tag":a.inbound_tag,"settings":{"clients":[{"email":"web-a","id":"changed"}]}}]}),
        confirmation: None,
        expected_revision: None,
    };
    let e = effects(&[a], "n", &r);
    assert_eq!(e[0].native_user["id"], "changed");
    assert!(e[0].present);
}

#[test]
fn same_email_with_a_different_live_credential_is_not_reconciled_as_success() {
    let expected = json!({"email":"same","id":"expected"});
    let actual = json!({"email":"same","account":{"type":"xray.proxy.vless.Account","value":{"id":"other"}}});
    assert!(!runtime_credentials_match(&expected, &actual));
}
