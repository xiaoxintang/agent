#[cfg(target_os = "linux")]
use panel_common::ServiceReport;
use panel_common::{ManagedService, ServiceError, TaskOutcome};

pub async fn status(service: ManagedService) -> TaskOutcome {
    if crate::security::require_unprivileged().is_err() {
        return TaskOutcome::Unavailable {
            reason: ServiceError::PrivilegeDenied,
        };
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = service;
        TaskOutcome::Unavailable {
            reason: ServiceError::UnsupportedPlatform,
        }
    }
    #[cfg(target_os = "linux")]
    {
        match tokio::time::timeout(std::time::Duration::from_secs(3), query(service)).await {
            Ok(Ok(report)) => TaskOutcome::ServiceStatus(report),
            Ok(Err(reason)) => TaskOutcome::Unavailable { reason },
            Err(_) => TaskOutcome::Unavailable {
                reason: ServiceError::Timeout,
            },
        }
    }
}

#[cfg(target_os = "linux")]
async fn query(service: ManagedService) -> Result<ServiceReport, ServiceError> {
    let connection = zbus::Connection::system().await.map_err(map_error)?;
    let manager = zbus::Proxy::new(
        &connection,
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
    )
    .await
    .map_err(map_error)?;
    let path: zbus::OwnedObjectPath = match manager.call("GetUnit", &(service.unit_name(),)).await {
        Ok(path) => path,
        Err(zbus::Error::MethodError(name, _, _))
            if name.as_str() == "org.freedesktop.systemd1.NoSuchUnit" =>
        {
            return Ok(ServiceReport {
                service,
                load_state: "not_loaded".into(),
                active_state: None,
                sub_state: None,
            });
        }
        Err(error) => return Err(map_error(error)),
    };
    let unit: zbus::Proxy<'_> = zbus::proxy::Builder::new(&connection)
        .destination("org.freedesktop.systemd1")
        .path(path)
        .interface("org.freedesktop.systemd1.Unit")
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
        .map_err(map_error)?;
    let report = ServiceReport {
        service,
        load_state: unit.get_property("LoadState").await.map_err(map_error)?,
        active_state: Some(unit.get_property("ActiveState").await.map_err(map_error)?),
        sub_state: Some(unit.get_property("SubState").await.map_err(map_error)?),
    };
    if !report.valid() {
        return Err(ServiceError::SystemdUnavailable);
    }
    Ok(report)
}

#[cfg(target_os = "linux")]
fn map_error(error: zbus::Error) -> ServiceError {
    match error {
        zbus::Error::FDO(error)
            if matches!(
                *error,
                zbus::fdo::Error::AccessDenied(_) | zbus::fdo::Error::AuthFailed(_)
            ) =>
        {
            ServiceError::AccessDenied
        }
        _ => ServiceError::SystemdUnavailable,
    }
}

#[cfg(target_os = "linux")]
pub async fn xray_invocation() -> Result<String, ServiceError> {
    let connection = zbus::Connection::system().await.map_err(map_error)?;
    let unit: zbus::Proxy<'_> = zbus::proxy::Builder::new(&connection)
        .destination("org.freedesktop.systemd1")
        .path("/org/freedesktop/systemd1/unit/xray_2eservice")
        .interface("org.freedesktop.systemd1.Unit")
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
        .map_err(map_error)?;
    let id: Vec<u8> = unit.get_property("InvocationID").await.map_err(map_error)?;
    if id.len() != 16 || id.iter().all(|byte| *byte == 0) {
        return Err(ServiceError::SystemdUnavailable);
    }
    Ok(hex::encode(id))
}
