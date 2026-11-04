use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, TS)]
pub enum XrayVersion {
    #[serde(rename = "26.10.21")]
    V26_10_21,
    #[serde(rename = "26.10.31")]
    V26_10_31,
}
impl XrayVersion {
    pub fn name(self) -> &'static str {
        match self {
            Self::V26_10_21 => "26.10.21",
            Self::V26_10_31 => "26.10.31",
        }
    }
    pub fn artifact(self, arch: &str) -> Option<(&'static str, &'static str)> {
        match (self, arch) {
            (Self::V26_10_21, "x86_64") => Some((
                "Xray-linux-64.zip",
                "0eec3ed4163c273d8c8bf821f7d219e257921cf584aa3fb970715e458e631978",
            )),
            (Self::V26_10_21, "aarch64") => Some((
                "Xray-linux-arm64-v8a.zip",
                "d1304946c486d98e6b886e9b931eaaaebb23835b30e217638037b7f2721d4e9c",
            )),
            (Self::V26_10_31, "x86_64") => Some((
                "Xray-linux-64.zip",
                "e4bd9119f50ae7c41658b1abd0679ae06f98bd864a2907ff33f2f2cf7081b214",
            )),
            (Self::V26_10_31, "aarch64") => Some((
                "Xray-linux-arm64-v8a.zip",
                "1811c8f1da0c3b6dc707ebaf1c8390e987ba39859a8f0a7aadd8526157f2ec8d",
            )),
            _ => None,
        }
    }
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, TS)]
#[serde(deny_unknown_fields)]
pub struct XrayRelease {
    pub version: XrayVersion,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, TS)]
#[serde(rename_all = "snake_case")]
pub enum XrayOperation {
    Install(XrayRelease),
    Upgrade(XrayRelease),
    Start,
    Stop,
    Restart,
    Rollback,
    Uninstall,
    Status,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, TS)]
pub struct XrayReport {
    pub installed: bool,
    pub version: Option<XrayVersion>,
    pub previous_version: Option<XrayVersion>,
    pub active: bool,
}
impl XrayReport {
    pub fn valid_for(&self, operation: &XrayOperation) -> bool {
        if self.installed != self.version.is_some()
            || (!self.installed && (self.active || self.previous_version.is_some()))
        {
            return false;
        }
        match operation {
            XrayOperation::Install(release) | XrayOperation::Upgrade(release) => {
                self.installed && self.version == Some(release.version)
            }
            XrayOperation::Start | XrayOperation::Restart => self.installed && self.active,
            XrayOperation::Stop => self.installed && !self.active,
            XrayOperation::Uninstall => !self.installed,
            XrayOperation::Rollback => self.installed,
            XrayOperation::Status => true,
        }
    }
}
#[derive(Deserialize, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct XrayApiInput {
    pub idempotency_key: String,
    pub version: Option<XrayVersion>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, TS)]
#[serde(rename_all = "snake_case")]
pub enum XrayError {
    UnsupportedPlatform,
    HelperUnavailable,
    Unauthorized,
    UnownedInstallation,
    InvalidState,
    VerificationFailed,
    DownloadFailed,
    ServiceFailed,
    RollbackFailed,
    Interrupted,
    Expired,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, TS)]
#[serde(tag = "status", content = "data", rename_all = "snake_case")]
pub enum XrayResult {
    Ok(XrayReport),
    Error(XrayError),
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HelperRequest {
    pub task_id: String,
    pub deadline_at: i64,
    pub operation: HelperOperation,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HelperOperation {
    Lifecycle(XrayOperation),
    ApplyConfig(crate::configuration::ConfigSubmission),
}
