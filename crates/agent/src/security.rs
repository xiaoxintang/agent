use crate::AgentError;

pub fn require_unprivileged() -> Result<(), AgentError> {
    #[cfg(unix)]
    check_uid(rustix::process::geteuid().as_raw())?;
    Ok(())
}

pub fn harden_process() -> Result<(), AgentError> {
    require_unprivileged()?;
    #[cfg(target_os = "linux")]
    rustix::thread::set_no_new_privs(true).map_err(|_| AgentError::PrivilegeDenied)?;
    Ok(())
}

#[cfg(any(unix, test))]
fn check_uid(uid: u32) -> Result<(), AgentError> {
    if uid == 0 {
        Err(AgentError::PrivilegeDenied)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn root_is_refused_and_normal_user_is_accepted() {
        assert_eq!(check_uid(0), Err(AgentError::PrivilegeDenied));
        assert!(check_uid(1001).is_ok());
    }
}
