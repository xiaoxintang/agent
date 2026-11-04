use panel_common::MetricsSnapshot;
#[cfg(not(target_os = "linux"))]
use panel_common::{MetricIssue, MetricName, MetricReason};

#[derive(Default)]
pub struct Collector {
    #[cfg(target_os = "linux")]
    cpu: Option<linux::CpuTicks>,
    #[cfg(target_os = "linux")]
    network: Option<(std::time::Instant, linux::Counters)>,
}

impl Collector {
    pub fn sample(&mut self) -> MetricsSnapshot {
        let mut snapshot = MetricsSnapshot {
            sampled_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("UTC clock")
                .as_secs() as i64,
            cpu_percent: None,
            memory: None,
            disk: None,
            network: None,
            issues: Vec::new(),
        };
        #[cfg(target_os = "linux")]
        linux::sample(self, &mut snapshot);
        #[cfg(not(target_os = "linux"))]
        for metric in [
            MetricName::Cpu,
            MetricName::Memory,
            MetricName::Disk,
            MetricName::Network,
        ] {
            snapshot.issues.push(MetricIssue {
                metric,
                reason: MetricReason::UnsupportedPlatform,
            });
        }
        snapshot
    }
}

#[cfg(any(target_os = "linux", test))]
mod linux {
    use panel_common::{ByteUsage, MetricReason, NetworkUsage};
    use std::collections::BTreeMap;

    pub(super) struct CpuTicks {
        total: u64,
        idle: u64,
    }
    pub(super) type Counters = BTreeMap<String, (u64, u64)>;

    fn cpu(input: &str) -> Result<CpuTicks, MetricReason> {
        let mut fields = input.split_whitespace();
        if fields.next() != Some("cpu") {
            return Err(MetricReason::InvalidData);
        }
        let values = fields
            .take(8)
            .map(|field| field.parse::<u64>().map_err(|_| MetricReason::InvalidData))
            .collect::<Result<Vec<_>, _>>()?;
        if values.len() < 4 {
            return Err(MetricReason::InvalidData);
        }
        let total = values
            .iter()
            .try_fold(0_u64, |sum, value| sum.checked_add(*value))
            .ok_or(MetricReason::InvalidData)?;
        let idle = values[3]
            .checked_add(values.get(4).copied().unwrap_or(0))
            .ok_or(MetricReason::InvalidData)?;
        Ok(CpuTicks { total, idle })
    }
    fn cpu_percent(previous: &CpuTicks, current: &CpuTicks) -> Option<f64> {
        let total = current.total.checked_sub(previous.total)?;
        let idle = current.idle.checked_sub(previous.idle)?;
        let busy = total.checked_sub(idle)?;
        (total > 0).then(|| busy as f64 / total as f64 * 100.0)
    }
    fn memory(input: &str) -> Result<ByteUsage, MetricReason> {
        let mut total = None;
        let mut available = None;
        for line in input.lines() {
            let mut fields = line.split_whitespace();
            let target = match fields.next() {
                Some("MemTotal:") => &mut total,
                Some("MemAvailable:") => &mut available,
                _ => continue,
            };
            let value = fields
                .next()
                .and_then(|value| value.parse::<u64>().ok())
                .and_then(|value| value.checked_mul(1024))
                .ok_or(MetricReason::InvalidData)?;
            if fields.next() != Some("kB") {
                return Err(MetricReason::InvalidData);
            }
            *target = Some(value);
        }
        let total = total.ok_or(MetricReason::InvalidData)?;
        let available = available.ok_or(MetricReason::InvalidData)?;
        let used = total
            .checked_sub(available)
            .ok_or(MetricReason::InvalidData)?;
        usage(total, used, available)
    }
    fn usage(total: u64, used: u64, available: u64) -> Result<ByteUsage, MetricReason> {
        let result = ByteUsage {
            total_bytes: total.to_string(),
            used_bytes: used.to_string(),
            available_bytes: available.to_string(),
        };
        if !result.valid() {
            return Err(MetricReason::InvalidData);
        }
        Ok(result)
    }
    fn disk(
        blocks: u64,
        free: u64,
        available: u64,
        fragment: u64,
    ) -> Result<ByteUsage, MetricReason> {
        let total = blocks
            .checked_mul(fragment)
            .ok_or(MetricReason::InvalidData)?;
        let used = blocks
            .checked_sub(free)
            .and_then(|value| value.checked_mul(fragment))
            .ok_or(MetricReason::InvalidData)?;
        let available = available
            .checked_mul(fragment)
            .ok_or(MetricReason::InvalidData)?;
        usage(total, used, available)
    }
    fn network(input: &str) -> Result<Counters, MetricReason> {
        let mut counters = BTreeMap::new();
        for line in input.lines().skip(2) {
            let (name, fields) = line.rsplit_once(':').ok_or(MetricReason::InvalidData)?;
            let name = name.trim();
            if name == "lo" {
                continue;
            }
            if name.is_empty() || name.len() > 64 || name.chars().any(char::is_control) {
                return Err(MetricReason::InvalidData);
            }
            if counters.len() >= 128 {
                return Err(MetricReason::LimitExceeded);
            }
            let values = fields
                .split_whitespace()
                .take(16)
                .map(|value| value.parse::<u64>().map_err(|_| MetricReason::InvalidData))
                .collect::<Result<Vec<_>, _>>()?;
            if values.len() != 16
                || counters
                    .insert(name.to_owned(), (values[0], values[8]))
                    .is_some()
            {
                return Err(MetricReason::InvalidData);
            }
        }
        Ok(counters)
    }
    fn network_usage(
        current: &Counters,
        previous: Option<(&Counters, f64)>,
    ) -> Result<NetworkUsage, MetricReason> {
        let (received, transmitted) = current
            .values()
            .try_fold((0_u64, 0_u64), |(rx, tx), (a, b)| {
                Some((rx.checked_add(*a)?, tx.checked_add(*b)?))
            })
            .ok_or(MetricReason::InvalidData)?;
        let rates = previous.and_then(|(previous, seconds)| {
            if seconds <= 0.0 || !seconds.is_finite() || current.len() != previous.len() {
                return None;
            }
            let (rx, tx) =
                current
                    .iter()
                    .try_fold((0_u64, 0_u64), |(rx, tx), (name, (a, b))| {
                        let (old_a, old_b) = previous.get(name)?;
                        Some((
                            rx.checked_add(a.checked_sub(*old_a)?)?,
                            tx.checked_add(b.checked_sub(*old_b)?)?,
                        ))
                    })?;
            Some((rx as f64 / seconds, tx as f64 / seconds))
        });
        Ok(NetworkUsage {
            received_bytes: received.to_string(),
            transmitted_bytes: transmitted.to_string(),
            received_bytes_per_second: rates.map(|rates| rates.0),
            transmitted_bytes_per_second: rates.map(|rates| rates.1),
        })
    }

    #[cfg(target_os = "linux")]
    fn read(path: &str, limit: u64) -> Result<String, MetricReason> {
        use std::io::Read;
        let mut text = String::new();
        std::fs::File::open(path)
            .map_err(|_| MetricReason::ReadFailed)?
            .take(limit + 1)
            .read_to_string(&mut text)
            .map_err(|_| MetricReason::ReadFailed)?;
        if text.len() as u64 > limit {
            return Err(MetricReason::LimitExceeded);
        }
        Ok(text)
    }
    #[cfg(target_os = "linux")]
    pub(super) fn sample(
        collector: &mut super::Collector,
        snapshot: &mut panel_common::MetricsSnapshot,
    ) {
        use panel_common::{MetricIssue, MetricName};
        use std::io::{BufRead, Read};
        let cpu = (|| {
            let file = std::fs::File::open("/proc/stat").map_err(|_| MetricReason::ReadFailed)?;
            let mut first = String::new();
            std::io::BufReader::new(file.take(1024))
                .read_line(&mut first)
                .map_err(|_| MetricReason::ReadFailed)?;
            if !first.ends_with('\n') {
                return Err(MetricReason::LimitExceeded);
            }
            cpu(&first)
        })();
        match cpu {
            Ok(current) => {
                snapshot.cpu_percent = collector
                    .cpu
                    .as_ref()
                    .and_then(|old| cpu_percent(old, &current));
                collector.cpu = Some(current);
            }
            Err(reason) => {
                collector.cpu = None;
                snapshot.issues.push(MetricIssue {
                    metric: MetricName::Cpu,
                    reason,
                });
            }
        }
        match read("/proc/meminfo", 65536).and_then(|text| memory(&text)) {
            Ok(value) => snapshot.memory = Some(value),
            Err(reason) => snapshot.issues.push(MetricIssue {
                metric: MetricName::Memory,
                reason,
            }),
        }
        match rustix::fs::statvfs("/")
            .map_err(|_| MetricReason::ReadFailed)
            .and_then(|value| {
                disk(
                    value.f_blocks,
                    value.f_bfree,
                    value.f_bavail,
                    value.f_frsize,
                )
            }) {
            Ok(value) => snapshot.disk = Some(value),
            Err(reason) => snapshot.issues.push(MetricIssue {
                metric: MetricName::Disk,
                reason,
            }),
        }
        let now = std::time::Instant::now();
        match read("/proc/net/dev", 65536)
            .and_then(|text| network(&text))
            .and_then(|counters| {
                let usage = network_usage(
                    &counters,
                    collector
                        .network
                        .as_ref()
                        .map(|(at, counters)| (counters, now.duration_since(*at).as_secs_f64())),
                )?;
                collector.network = Some((now, counters));
                Ok(usage)
            }) {
            Ok(value) => snapshot.network = Some(value),
            Err(reason) => {
                collector.network = None;
                snapshot.issues.push(MetricIssue {
                    metric: MetricName::Network,
                    reason,
                });
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn cpu_uses_deltas_without_counting_guest_twice() {
            let before = cpu("cpu 100 0 50 850 0 0 0 0 20 0").unwrap();
            let after = cpu("cpu 120 0 70 910 0 0 0 0 30 0").unwrap();
            assert_eq!(cpu_percent(&before, &after), Some(40.0));
            assert!(cpu_percent(&after, &before).is_none());
            assert!(cpu_percent(&before, &before).is_none());
        }
        #[test]
        fn memory_disk_and_reserved_space_are_real_byte_quantities() {
            let memory = memory("MemTotal: 1024 kB\nMemAvailable: 256 kB\n").unwrap();
            assert_eq!(memory.used_bytes, "786432");
            assert!(super::memory("MemTotal: 1 kB\nMemAvailable: 2 kB\n").is_err());
            let disk = disk(100, 40, 30, 4096).unwrap();
            assert_eq!(disk.used_bytes, "245760");
            assert_eq!(disk.available_bytes, "122880");
        }
        #[test]
        fn network_reset_and_interface_changes_never_create_false_rates() {
            let text = "header\nheader\n lo: 9999 0 0 0 0 0 0 0 9999 0 0 0 0 0 0 0\n eth0: 100 0 0 0 0 0 0 0 200 0 0 0 0 0 0 0\n";
            let previous = network(text).unwrap();
            assert_eq!(previous.len(), 1);
            let current = network(
                &text
                    .replace("eth0: 100", "eth0: 200")
                    .replace("0 200 0", "0 400 0"),
            )
            .unwrap();
            let usage = network_usage(&current, Some((&previous, 5.0))).unwrap();
            assert_eq!(usage.received_bytes_per_second, Some(20.0));
            assert_eq!(usage.transmitted_bytes_per_second, Some(40.0));
            assert!(
                network_usage(&previous, Some((&current, 5.0)))
                    .unwrap()
                    .received_bytes_per_second
                    .is_none()
            );
            assert!(
                network_usage(&current, None)
                    .unwrap()
                    .received_bytes_per_second
                    .is_none()
            );
            let mut changed = current.clone();
            changed.insert("eth1".into(), (1000, 1000));
            assert!(
                network_usage(&changed, Some((&current, 5.0)))
                    .unwrap()
                    .received_bytes_per_second
                    .is_none()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn platform_sampling_is_honest_and_bounded() {
        let snapshot = Collector::default().sample();
        assert!(snapshot.valid(snapshot.sampled_at));
        #[cfg(target_os = "linux")]
        {
            assert!(snapshot.memory.is_some());
            assert!(snapshot.disk.is_some());
            assert!(snapshot.network.is_some());
            assert!(snapshot.cpu_percent.is_none());
        }
        #[cfg(not(target_os = "linux"))]
        {
            assert_eq!(snapshot.issues.len(), 4);
            assert!(snapshot.memory.is_none());
            assert!(snapshot.cpu_percent.is_none());
        }
    }
}
