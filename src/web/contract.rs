use serde_json::{Value, json};

pub fn contract() -> Value {
    let mut value: Value = serde_json::from_str(include_str!("../../schemas/web-api.json"))
        .expect("valid Web contract");
    value["info"]["version"] = json!(env!("CARGO_PKG_VERSION"));
    value
}
