use crate::error::Result;
use std::path::Path;
use tokio::process::Command;

/// Spawn a child dev server process.
/// Sets PORT=<port> env var. Calls extra_args_for_port to inject framework flags.
pub async fn spawn_child(cwd: &Path, args: &[String], port: u16) -> Result<tokio::process::Child> {
    if args.is_empty() {
        return Err(crate::error::Error::Ipc(
            "No arguments provided to spawn_child".to_string(),
        ));
    }

    // Split args into program and rest
    let program = &args[0];
    let rest_args: Vec<&str> = args[1..].iter().map(|s| s.as_str()).collect();

    // Get extra args for this framework
    let extra = crate::detect::extra_args_for_port(cwd, &rest_args, port)?;

    // Spawn the child process
    let mut cmd = Command::new(program);
    cmd.args(&rest_args)
        .args(&extra)
        .env("PORT", port.to_string())
        .current_dir(cwd)
        .kill_on_drop(false);

    let child = cmd.spawn()?;
    Ok(child)
}

/// Gracefully stop a child process: SIGTERM, wait 5s, SIGKILL.
pub async fn stop_child(child: &mut tokio::process::Child) -> Result<()> {
    let pid = match child.id() {
        Some(id) => id,
        None => {
            // Already exited
            return Ok(());
        }
    };

    // Send SIGTERM
    #[cfg(unix)]
    {
        use nix::sys::signal::{kill, Signal};
        use nix::unistd::Pid;

        let _ = kill(Pid::from_raw(pid as i32), Signal::SIGTERM);
    }

    #[cfg(windows)]
    {
        let _ = child.kill().await;
    }

    // Wait up to 5 seconds for graceful shutdown
    let result = tokio::select! {
        _ = child.wait() => Ok(()),
        _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {
            // Timeout: force kill
            let _ = child.kill().await;
            Err(crate::error::Error::Ipc(
                "Process did not respond to SIGTERM, force killed".to_string(),
            ))
        }
    };

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn spawns_and_kills_child() {
        #[cfg(unix)]
        {
            let args = vec!["sleep".to_string(), "60".to_string()];
            let mut child = spawn_child(Path::new("/tmp"), &args, 4321)
                .await
                .expect("Failed to spawn child");

            let pid = child.id().expect("Failed to get child PID");

            // Let it start
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            // Verify it's alive
            assert!(crate::routes::pid_alive_check(pid));

            // Stop it
            let _ = stop_child(&mut child).await;

            // Wait a bit for process to actually die
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;

            // Verify it's dead
            assert!(!crate::routes::pid_alive_check(pid));
        }

        #[cfg(windows)]
        {
            let args = vec!["timeout".to_string(), "/T".to_string(), "60".to_string()];
            let mut child = spawn_child(Path::new("C:\\"), &args, 4321)
                .await
                .expect("Failed to spawn child");

            let pid = child.id().expect("Failed to get child PID");

            // Stop it
            let _ = stop_child(&mut child).await;

            // Wait a bit for process to actually die
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;

            // Verify it's dead
            assert!(!crate::routes::pid_alive_check(pid));
        }
    }

    #[tokio::test]
    async fn child_receives_port_env() {
        #[cfg(unix)]
        {
            use rand::Rng;

            let mut rng = rand::thread_rng();
            let random_id = rng.gen::<u32>();
            let test_file = format!("/tmp/portless_port_test_{}.txt", random_id);

            let args = vec![
                "sh".to_string(),
                "-c".to_string(),
                format!("echo $PORT > {}", test_file),
            ];

            let mut child = spawn_child(Path::new("/tmp"), &args, 4321)
                .await
                .expect("Failed to spawn child");

            // Wait for completion
            let _ = child.wait().await;

            // Read the file and verify
            if let Ok(content) = std::fs::read_to_string(&test_file) {
                let port_str = content.trim();
                assert_eq!(port_str, "4321");
            } else {
                panic!("Failed to read test file");
            }

            // Cleanup
            let _ = std::fs::remove_file(&test_file);
        }

        #[cfg(windows)]
        {
            use rand::Rng;

            let mut rng = rand::thread_rng();
            let random_id = rng.gen::<u32>();
            let test_file = format!("C:\\temp\\portless_port_test_{}.txt", random_id);

            let args = vec![
                "cmd".to_string(),
                "/C".to_string(),
                format!("echo %PORT% > {}", test_file),
            ];

            let mut child = spawn_child(Path::new("C:\\"), &args, 4321)
                .await
                .expect("Failed to spawn child");

            // Wait for completion
            let _ = child.wait().await;

            // Read the file and verify
            if let Ok(content) = std::fs::read_to_string(&test_file) {
                let port_str = content.trim();
                assert_eq!(port_str, "4321");
            } else {
                panic!("Failed to read test file");
            }

            // Cleanup
            let _ = std::fs::remove_file(&test_file);
        }
    }
}
