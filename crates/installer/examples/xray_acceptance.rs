#[cfg(target_os = "linux")]
mod linux {
    use panel_common::{XrayError, XrayOperation, XrayRelease, XrayVersion};
    use panel_installer::xray::lifecycle::{
        Layout, Manager, Service, ServiceAction, ServiceState, Systemd,
    };
    use std::{
        error::Error,
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };

    // Acceptance-only failure injection: stop the new service once after a real restart.
    struct FailureInjection {
        real: Systemd,
        stop_next_restart: Arc<AtomicUsize>,
        interrupted: Arc<tokio::sync::Notify>,
    }
    struct Cleanup {
        name: String,
        unit: PathBuf,
    }
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::process::Command::new("/usr/bin/systemctl")
                .args(["--user", "stop", &self.name])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            let _ = std::fs::remove_file(&self.unit);
            let _ = std::process::Command::new("/usr/bin/systemctl")
                .args(["--user", "daemon-reload"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }
    impl Service for FailureInjection {
        async fn inspect(&self) -> Result<ServiceState, XrayError> {
            self.real.inspect().await
        }
        async fn action(&self, action: ServiceAction) -> Result<(), XrayError> {
            self.real.action(action).await?;
            if matches!(action, ServiceAction::Restart) {
                match self.stop_next_restart.swap(0, Ordering::SeqCst) {
                    1 => self.real.action(ServiceAction::Stop).await?,
                    2 => {
                        self.interrupted.notify_one();
                        std::future::pending::<()>().await;
                    }
                    _ => (),
                }
            }
            Ok(())
        }
    }
    pub async fn run() -> Result<(), Box<dyn Error>> {
        if rustix::process::geteuid().as_raw() == 0 {
            return Err("Run acceptance as an ordinary user".into());
        }
        let directory = tempfile::Builder::new()
            .prefix("panel-xray-phase4-")
            .tempdir()?;
        let root = directory.path().join("managed");
        let runtime =
            PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").ok_or("XDG_RUNTIME_DIR required")?);
        let unit_directory = runtime.join("systemd/user");
        std::fs::create_dir_all(&unit_directory)?;
        let name = format!("panel-phase4-{}.service", uuid::Uuid::new_v4());
        let unit = unit_directory.join(&name);
        let _cleanup = Cleanup {
            name: name.clone(),
            unit: unit.clone(),
        };
        let stop_next_restart = Arc::new(AtomicUsize::new(0));
        let interrupted = Arc::new(tokio::sync::Notify::new());
        let manager = Manager {
            layout: Layout {
                root: root.clone(),
                control: directory.path().join("control"),
                unit: unit.clone(),
            },
            service: FailureInjection {
                real: Systemd::acceptance(&name).map_err(debug)?,
                stop_next_restart: stop_next_restart.clone(),
                interrupted: interrupted.clone(),
            },
            user_scope: true,
        };
        let result = async {
            assert!(!manager.operate(XrayOperation::Status).await.map_err(debug)?.installed);
            println!("acceptance: fresh status checked");
            let installed = manager.operate(XrayOperation::Install(XrayRelease { version: XrayVersion::V26_10_21 })).await.map_err(debug)?;
            assert!(installed.active && installed.version == Some(XrayVersion::V26_10_21));
            println!("acceptance: first version installed");
            manager.operate(XrayOperation::Restart).await.map_err(|error| format!("restart failed: {error:?}"))?;
            println!("acceptance: restarted");
            assert!(!manager.operate(XrayOperation::Stop).await.map_err(debug)?.active);
            println!("acceptance: stopped");
            assert!(manager.operate(XrayOperation::Start).await.map_err(debug)?.active);
            println!("acceptance: started again");
            let upgraded = manager.operate(XrayOperation::Upgrade(XrayRelease { version: XrayVersion::V26_10_31 })).await.map_err(debug)?;
            assert!(upgraded.active && upgraded.version == Some(XrayVersion::V26_10_31) && upgraded.previous_version == Some(XrayVersion::V26_10_21));
            println!("acceptance: second version upgraded");
            assert_eq!(manager.operate(XrayOperation::Rollback).await.map_err(debug)?.version, Some(XrayVersion::V26_10_21));
            stop_next_restart.store(1, Ordering::SeqCst);
            assert_eq!(manager.operate(XrayOperation::Upgrade(XrayRelease { version: XrayVersion::V26_10_31 })).await.unwrap_err(), XrayError::ServiceFailed);
            let recovered = manager.operate(XrayOperation::Status).await.map_err(debug)?;
            assert!(recovered.active && recovered.version == Some(XrayVersion::V26_10_21));
            stop_next_restart.store(2, Ordering::SeqCst);
            {
                let mut operation = Box::pin(manager.operate(XrayOperation::Upgrade(XrayRelease { version: XrayVersion::V26_10_31 })));
                tokio::time::timeout(std::time::Duration::from_secs(120), async {
                    tokio::select! { _ = interrupted.notified() => (), result = &mut operation => panic!("expected interruption checkpoint, got {result:?}") }
                }).await?;
            }
            let recovered = manager.operate(XrayOperation::Status).await.map_err(debug)?;
            assert!(recovered.active && recovered.version == Some(XrayVersion::V26_10_21));
            assert!(!manager.layout.control.join("switch.json").exists());
            use panel_common::configuration::{NodeConfig, ConfigInbound, ConfigClient, ConfigRef, ConfigSubmission};
            let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
            let port = listener.local_addr()?.port(); drop(listener);
            let config = NodeConfig { inbounds: vec![ConfigInbound { id: uuid::Uuid::new_v4().to_string(), listen_address: "127.0.0.1".into(), listen_port: port, target: "www.example.com:443".into(), server_names: vec!["www.example.com".into()], private_key: "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc".into(), short_ids: vec!["abcd".into()], clients: vec![ConfigClient {id: uuid::Uuid::new_v4().to_string(), protocol_id: uuid::Uuid::new_v4().to_string()}] }] };
            let submit = |revision, config: NodeConfig| ConfigSubmission { reference: ConfigRef {revision,content_hash: config.content_hash().unwrap()},config };
            assert!(manager.apply_config(submit(1, config.clone())).await.map_err(debug)?.active);
            assert!(tokio::net::TcpStream::connect(("127.0.0.1",port)).await.is_ok());
            let healthy = std::fs::read(root.join("config.json"))?;
            let mut invalid = config.clone(); invalid.inbounds[0].private_key = "invalid".into();
            assert_eq!(manager.apply_config(ConfigSubmission {reference: ConfigRef {revision:2,content_hash:"0".repeat(64)},config: invalid}).await.unwrap_err(), XrayError::VerificationFailed);
            assert_eq!(std::fs::read(root.join("config.json"))?, healthy);
            let occupied = std::net::TcpListener::bind("127.0.0.1:0")?;
            let mut bad = config.clone(); bad.inbounds[0].listen_port = occupied.local_addr()?.port();
            assert_eq!(manager.apply_config(submit(2,bad)).await.unwrap_err(), XrayError::ServiceFailed);
            assert_eq!(std::fs::read(root.join("config.json"))?, healthy);
            assert!(manager.operate(XrayOperation::Status).await.map_err(debug)?.active);
            stop_next_restart.store(2, Ordering::SeqCst);
            {
                let mut operation = Box::pin(manager.apply_config(submit(3,config.clone())));
                tokio::time::timeout(std::time::Duration::from_secs(30), async {
                    tokio::select! { _ = interrupted.notified() => (), result = &mut operation => panic!("expected configuration interruption, got {result:?}") }
                }).await?;
            }
            assert!(manager.operate(XrayOperation::Status).await.map_err(debug)?.active);
            assert_eq!(std::fs::read(root.join("config.json"))?, healthy);
            assert!(!manager.layout.control.join("config-switch.json").exists());
            assert!(manager.apply_config(submit(4,config)).await.map_err(debug)?.active);
            println!("acceptance: real VLESS/REALITY apply, invalid input, occupied-port rollback and interruption recovery passed");
            let removed = manager.operate(XrayOperation::Uninstall).await.map_err(debug)?;
            assert!(!removed.installed && !removed.active);
            assert!(root.join("config.json").exists());
            assert!(!root.join("current").exists() && !unit.exists());
            let foreign = directory.path().join("foreign"); std::fs::create_dir(&foreign)?;
            std::fs::write(foreign.join("xray"), b"unowned sentinel")?;
            let foreign_manager = Manager { layout: Layout { root: foreign.clone(), control: directory.path().join("foreign-control"), unit: unit_directory.join(format!("panel-phase4-{}.service", uuid::Uuid::new_v4())) }, service: Systemd::acceptance(&format!("panel-phase4-{}.service", uuid::Uuid::new_v4())).map_err(debug)?, user_scope: true };
            assert_eq!(foreign_manager.operate(XrayOperation::Install(XrayRelease { version: XrayVersion::V26_10_31 })).await.unwrap_err(), XrayError::UnownedInstallation);
            assert_eq!(std::fs::read(foreign.join("xray"))?, b"unowned sentinel");
            println!("{{\"xray_lifecycle\":\"passed\",\"real_versions\":[\"26.10.21\",\"26.10.31\"],\"systemd_scope\":\"user\",\"install_restart_stop_start_upgrade_rollback_uninstall\":true,\"failed_upgrade_restores_old_version\":true,\"unowned_installation_preserved\":true}}");
            Ok::<_, Box<dyn Error>>(())
        }.await;
        let _ = manager.service.action(ServiceAction::Stop).await;
        if unit.exists() {
            std::fs::remove_file(&unit)?;
        }
        let _ = manager.service.action(ServiceAction::Reload).await;
        result
    }
    fn debug(error: XrayError) -> Box<dyn Error> {
        format!("Xray acceptance error: {error:?}").into()
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "linux")]
    return linux::run().await;
    #[cfg(not(target_os = "linux"))]
    Err("Real Linux/user systemd is required".into())
}
