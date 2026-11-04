use panel_common::traffic::TrafficReport;
#[cfg(target_os = "linux")]
use panel_common::traffic::TrafficSample;

pub async fn sample() -> TrafficReport {
    if crate::security::require_unprivileged().is_err() {
        return TrafficReport::Unavailable;
    }
    #[cfg(target_os = "linux")]
    {
        match tokio::time::timeout(std::time::Duration::from_secs(3), async {
            let instance_id = crate::systemd::xray_invocation().await.ok()?;
            let counters = panel_common::stats::counters().await.ok()?;
            if crate::systemd::xray_invocation().await.ok()? != instance_id {
                return None;
            }
            let collected_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_secs() as i64;
            let sample = TrafficSample {
                instance_id,
                collected_at,
                counters,
            };
            sample.valid(collected_at).then_some(sample)
        })
        .await
        {
            Ok(Some(sample)) => TrafficReport::Available(sample),
            _ => TrafficReport::Unavailable,
        }
    }
    #[cfg(not(target_os = "linux"))]
    TrafficReport::Unavailable
}
