use crate::error::Result;
use std::path::Path;

/// Write the current PID to a file.
pub fn write_pid_file(path: &Path, pid: u32) -> Result<()> {
    std::fs::write(path, pid.to_string())?;
    Ok(())
}

/// Check if the daemon is already running by reading the PID file.
pub fn daemon_already_running(pid_path: &Path) -> bool {
    if !pid_path.exists() {
        return false;
    }
    let contents = match std::fs::read_to_string(pid_path) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let pid: u32 = match contents.trim().parse() {
        Ok(p) => p,
        Err(_) => return false,
    };
    crate::routes::pid_alive_check(pid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn pid_file_written_after_daemonize_preparation() {
        let temp = TempDir::new().unwrap();
        let pid_path = temp.path().join("daemon.pid");
        let my_pid = std::process::id();

        write_pid_file(&pid_path, my_pid).unwrap();

        let contents = std::fs::read_to_string(&pid_path).unwrap();
        let parsed: u32 = contents.trim().parse().unwrap();
        assert_eq!(parsed, my_pid);
    }

    #[test]
    fn already_running_detected_via_pid_file() {
        let temp = TempDir::new().unwrap();
        let pid_path = temp.path().join("daemon.pid");
        let my_pid = std::process::id();

        write_pid_file(&pid_path, my_pid).unwrap();

        assert!(daemon_already_running(&pid_path));
    }

    #[test]
    fn stale_pid_file_not_detected_as_running() {
        let temp = TempDir::new().unwrap();
        let pid_path = temp.path().join("daemon.pid");

        // u32::MAX is not a valid PID
        write_pid_file(&pid_path, u32::MAX).unwrap();

        assert!(!daemon_already_running(&pid_path));
    }
}
