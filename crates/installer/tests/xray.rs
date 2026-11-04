use panel_common::{XrayError, XrayOperation, XrayRelease, XrayVersion};
use panel_installer::xray::{
    download,
    lifecycle::{Layout, Manager, Service, ServiceAction, ServiceState},
};
use std::sync::atomic::{AtomicUsize, Ordering};

struct TestService {
    calls: AtomicUsize,
}
impl Service for TestService {
    async fn inspect(&self) -> Result<ServiceState, XrayError> {
        Ok(ServiceState {
            active: false,
            fragment: None,
        })
    }
    async fn action(&self, _: ServiceAction) -> Result<(), XrayError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[test]
fn only_pinned_versions_and_architectures_can_be_selected() {
    assert!(XrayVersion::V26_10_31.artifact("aarch64").is_some());
    assert!(XrayVersion::V26_10_21.artifact("x86_64").is_some());
    assert!(XrayVersion::V26_10_31.artifact("arbitrary").is_none());
    for value in [
        r#"{"install":{"version":"latest"}}"#,
        r#"{"install":{"version":"../../private"}}"#,
        r#"{"install":{"version":"26.10.31","url":"http://evil.invalid"}}"#,
        r#"{"shell":"id"}"#,
    ] {
        assert!(serde_json::from_str::<XrayOperation>(value).is_err());
    }
    assert!(download::extract(b"corrupt", "0000").is_err());
}

#[test]
fn archives_are_verified_before_use_and_only_the_expected_binary_is_read() {
    use std::io::{Cursor, Write};
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    writer.start_file("../../sentinel", options).unwrap();
    writer.write_all(b"must never be extracted").unwrap();
    writer.start_file("xray", options).unwrap();
    writer
        .write_all(b"\x7fELFtest fixture, never executed")
        .unwrap();
    let archive = writer.finish().unwrap().into_inner();
    let digest = panel_installer::xray::lifecycle::hash(&archive);
    assert_eq!(
        download::extract(&archive, &digest).unwrap(),
        b"\x7fELFtest fixture, never executed"
    );
    let mut corrupt = archive;
    corrupt[0] ^= 1;
    assert_eq!(
        download::extract(&corrupt, &digest).unwrap_err(),
        XrayError::VerificationFailed
    );
}

#[tokio::test]
async fn preexisting_unowned_directories_are_never_touched() {
    let directory = tempfile::tempdir().unwrap();
    let parent = directory.path().canonicalize().unwrap();
    let root = parent.join("foreign");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("xray"), b"sentinel").unwrap();
    let manager = Manager {
        layout: Layout {
            root: root.clone(),
            control: parent.join("control"),
            unit: parent.join("xray.service"),
        },
        service: TestService {
            calls: AtomicUsize::new(0),
        },
        user_scope: true,
    };
    assert_eq!(
        manager
            .operate(XrayOperation::Install(XrayRelease {
                version: XrayVersion::V26_10_31
            }))
            .await
            .unwrap_err(),
        XrayError::UnownedInstallation
    );
    assert_eq!(std::fs::read(root.join("xray")).unwrap(), b"sentinel");
    assert_eq!(manager.service.calls.load(Ordering::SeqCst), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn symlinked_managed_roots_cannot_redirect_installation_or_uninstall() {
    let directory = tempfile::tempdir().unwrap();
    let parent = directory.path().canonicalize().unwrap();
    let foreign = parent.join("foreign");
    std::fs::create_dir(&foreign).unwrap();
    std::fs::write(foreign.join("sentinel"), b"keep").unwrap();
    let root = parent.join("managed");
    std::os::unix::fs::symlink(&foreign, &root).unwrap();
    let manager = Manager {
        layout: Layout {
            root,
            control: parent.join("control"),
            unit: parent.join("xray.service"),
        },
        service: TestService {
            calls: AtomicUsize::new(0),
        },
        user_scope: true,
    };
    assert_eq!(
        manager.operate(XrayOperation::Uninstall).await.unwrap_err(),
        XrayError::UnownedInstallation
    );
    assert_eq!(std::fs::read(foreign.join("sentinel")).unwrap(), b"keep");
    assert_eq!(manager.service.calls.load(Ordering::SeqCst), 0);
}
