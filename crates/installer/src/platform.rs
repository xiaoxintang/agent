use std::{fs, path::Path, process::Command};
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
pub fn detect() -> Result<()> {
    if !cfg!(target_os = "linux") {
        return Err("Installation requires Linux".into());
    }
    #[cfg(unix)]
    if rustix::process::geteuid().as_raw() != 0 {
        return Err("Installation requires root (run this nonresident CLI through sudo)".into());
    }
    if !matches!(std::env::consts::ARCH, "x86_64" | "aarch64") {
        return Err("Only x86_64/aarch64 are supported".into());
    }
    let os = fs::read_to_string("/etc/os-release")?;
    if !os.lines().any(|line| {
        matches!(
            line,
            "ID=ubuntu" | "ID=debian" | "ID=\"ubuntu\"" | "ID=\"debian\""
        )
    }) {
        return Err("Only Debian/Ubuntu are supported".into());
    }
    if !Path::new("/run/systemd/system").is_dir()
        || fs::read_to_string("/proc/1/comm")?.trim() != "systemd"
    {
        return Err("systemd must be PID 1".into());
    }
    for path in [
        "/usr/bin/systemctl",
        "/usr/bin/systemd-run",
        "/usr/sbin/useradd",
        "/usr/bin/id",
        "/usr/bin/install",
    ] {
        if !Path::new(path).is_file() {
            return Err(format!("Required system capability missing: {path}").into());
        }
    }
    let output = Command::new("/usr/bin/systemctl")
        .arg("--version")
        .output()?;
    let version = String::from_utf8(output.stdout)?
        .split_whitespace()
        .nth(1)
        .ok_or("Invalid systemd version")?
        .parse::<u32>()?;
    if version < 249 {
        return Err("systemd >=249 is required for credential isolation".into());
    }
    #[cfg(unix)]
    {
        let stat = rustix::fs::statvfs("/var/lib")?;
        if stat.f_bavail.saturating_mul(stat.f_frsize) < 512 * 1024 * 1024 {
            return Err("At least 512 MiB free disk is required".into());
        }
    }
    Ok(())
}
