use crate::error::AppError;

pub(crate) fn report(
    event: &'static str,
    error: &AppError,
    request_id: Option<&str>,
    node_id: Option<&str>,
    task_id: Option<&str>,
) {
    eprintln!("{}", entry(event, error, request_id, node_id, task_id));
}

fn entry(
    event: &'static str,
    error: &AppError,
    request_id: Option<&str>,
    node_id: Option<&str>,
    task_id: Option<&str>,
) -> serde_json::Value {
    let id = |value: Option<&str>| {
        value
            .and_then(|id| uuid::Uuid::parse_str(id).ok())
            .map(|id| id.to_string())
    };
    serde_json::json!({
        "event": event,
        "status": error.status.as_u16(),
        "error_code": error.code,
        "database": error.database,
        "request_id": id(request_id),
        "node_id": id(node_id),
        "task_id": id(task_id),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn database_diagnostics_keep_context_without_database_messages_or_sql() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql("CREATE TABLE secret (value TEXT); CREATE TRIGGER fail BEFORE INSERT ON secret BEGIN SELECT RAISE(ABORT, 'private-password-token'); END;")
            .execute(&pool).await.unwrap();
        let error = sqlx::query("INSERT INTO secret VALUES (?)")
            .bind("private-config")
            .execute(&pool)
            .await
            .unwrap_err();
        let error = AppError::from(error);
        let request = uuid::Uuid::new_v4().to_string();
        let node = uuid::Uuid::new_v4().to_string();
        let task = uuid::Uuid::new_v4().to_string();
        let log = entry(
            "http_error",
            &error,
            Some(&request),
            Some(&node),
            Some(&task),
        );
        assert_eq!(log["database"]["kind"], "database");
        assert!(log["database"]["code"].is_string());
        assert_eq!(log["request_id"], request);
        assert_eq!(log["node_id"], node);
        assert_eq!(log["task_id"], task);
        let invalid = entry(
            "agent_task_error",
            &error,
            None,
            None,
            Some("private-token-as-task-id"),
        );
        assert!(invalid["task_id"].is_null());
        assert!(!invalid.to_string().contains("private-token-as-task-id"));
        for secret in [
            "private-password-token",
            "private-config",
            "INSERT",
            "secret",
        ] {
            assert!(!log.to_string().contains(secret));
        }
        pool.close().await;
    }
}
