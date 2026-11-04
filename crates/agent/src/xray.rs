#[cfg(target_os = "linux")]
use panel_common::xray::HelperRequest;
use panel_common::{TaskCommand, TaskOutcome, XrayError, XrayResult};

pub async fn execute(task: &TaskCommand, operation: panel_common::XrayOperation) -> TaskOutcome {
    if crate::security::require_unprivileged().is_err() {
        return TaskOutcome::Xray(XrayResult::Error(XrayError::Unauthorized));
    }
    #[cfg(target_os = "linux")]
    {
        TaskOutcome::Xray(
            match call(
                task,
                panel_common::xray::HelperOperation::Lifecycle(operation),
            )
            .await
            {
                Ok(result) => result,
                Err(error) => XrayResult::Error(error),
            },
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (task, operation);
        TaskOutcome::Xray(XrayResult::Error(XrayError::UnsupportedPlatform))
    }
}

pub async fn apply_config(
    task: &TaskCommand,
    reference: &panel_common::configuration::ConfigRef,
) -> TaskOutcome {
    let error = |error| TaskOutcome::Xray(XrayResult::Error(error));
    if crate::security::require_unprivileged().is_err() {
        return error(XrayError::Unauthorized);
    }
    let Some(config) = &task.private_config else {
        return error(XrayError::VerificationFailed);
    };
    if config.content_hash().ok().as_ref() != Some(&reference.content_hash) {
        return error(XrayError::VerificationFailed);
    }
    #[cfg(target_os = "linux")]
    {
        let submission = panel_common::configuration::ConfigSubmission {
            reference: reference.clone(),
            config: config.clone(),
        };
        match call(
            task,
            panel_common::xray::HelperOperation::ApplyConfig(submission),
        )
        .await
        {
            Ok(XrayResult::Ok(report)) if report.installed && report.active => {
                TaskOutcome::ConfigApplied(reference.clone())
            }
            Ok(XrayResult::Error(reason)) | Err(reason) => error(reason),
            _ => error(XrayError::ServiceFailed),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        error(XrayError::UnsupportedPlatform)
    }
}

#[cfg(target_os = "linux")]
async fn call(
    task: &TaskCommand,
    operation: panel_common::xray::HelperOperation,
) -> Result<XrayResult, XrayError> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::net::UnixStream::connect("/run/panel/xray-helper.sock"),
    )
    .await
    .map_err(|_| XrayError::HelperUnavailable)?
    .map_err(|_| XrayError::HelperUnavailable)?;
    if stream
        .peer_cred()
        .map_err(|_| XrayError::Unauthorized)?
        .uid()
        != 0
    {
        return Err(XrayError::Unauthorized);
    }
    let request = HelperRequest {
        task_id: task.id.clone(),
        deadline_at: task.deadline_at,
        operation,
    };
    let mut bytes = serde_json::to_vec(&request).map_err(|_| XrayError::InvalidState)?;
    bytes.push(b'\n');
    let mut output = Vec::new();
    tokio::time::timeout(std::time::Duration::from_secs(295), async {
        stream
            .write_all(&bytes)
            .await
            .map_err(|_| XrayError::HelperUnavailable)?;
        stream
            .shutdown()
            .await
            .map_err(|_| XrayError::HelperUnavailable)?;
        stream
            .take(4097)
            .read_to_end(&mut output)
            .await
            .map_err(|_| XrayError::HelperUnavailable)?;
        Ok::<_, XrayError>(())
    })
    .await
    .map_err(|_| XrayError::Interrupted)??;
    if output.len() > 4096 {
        return Err(XrayError::Unauthorized);
    }
    serde_json::from_slice(&output).map_err(|_| XrayError::HelperUnavailable)
}
