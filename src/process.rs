use crate::error::Result;
use std::path::Path;

/// Spawn a child dev server process.
/// Sets PORT=<port> and PORTAL_URL=https://<hostname> env vars.
/// Handles PortInjection variants for framework-specific port passing.
pub async fn spawn_child(
    cwd: &Path,
    args: &[String],
    port: u16,
    hostname: &str,
    injection: crate::detect::PortInjection,
) -> Result<tokio::process::Child> {
    if args.is_empty() {
        return Err(crate::error::Error::Ipc("No arguments provided to spawn_child".to_string()));
    }

    let port_str = port.to_string();

    // Substitute {port} in every arg
    let args: Vec<String> = args.iter()
        .map(|a| a.replace("{port}", &port_str))
        .collect();

    let program = &args[0];
    let rest: Vec<&str> = args[1..].iter().map(String::as_str).collect();

    let mut cmd = tokio::process::Command::new(program);
    cmd.env("PORT", &port_str)
        .env("PORTAL_URL", format!("https://{hostname}"))
        .current_dir(cwd)
        .kill_on_drop(false);

    match injection {
        crate::detect::PortInjection::EnvOnly => {
            cmd.args(&rest);
        }
        crate::detect::PortInjection::CliArgs(ref extra) => {
            cmd.args(&rest).args(extra);
        }
        crate::detect::PortInjection::AppendAddress(ref addr) => {
            cmd.args(&rest).arg(addr);
        }
    }

    Ok(cmd.spawn()?)
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
            let mut child = spawn_child(Path::new("/tmp"), &args, 4321, "test.localhost",
                crate::detect::PortInjection::EnvOnly)
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
            let mut child = spawn_child(Path::new("C:\\"), &args, 4321, "test.localhost",
                crate::detect::PortInjection::EnvOnly)
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
            let test_file = format!("/tmp/portal_port_test_{}.txt", random_id);

            let args = vec![
                "sh".to_string(),
                "-c".to_string(),
                format!("echo $PORT > {}", test_file),
            ];

            let mut child = spawn_child(Path::new("/tmp"), &args, 4321, "test.localhost",
                crate::detect::PortInjection::EnvOnly)
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
            let test_file = format!("C:\\temp\\portal_port_test_{}.txt", random_id);

            let args = vec![
                "cmd".to_string(),
                "/C".to_string(),
                format!("echo %PORT% > {}", test_file),
            ];

            let mut child = spawn_child(Path::new("C:\\"), &args, 4321, "test.localhost",
                crate::detect::PortInjection::EnvOnly)
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

    #[tokio::test]
    async fn child_receives_portal_url_env() {
        #[cfg(unix)]
        {
            use rand::Rng;
            let mut rng = rand::thread_rng();
            let random_id = rng.gen::<u32>();
            let test_file = format!("/tmp/portal_url_test_{}.txt", random_id);

            let args = vec![
                "sh".to_string(),
                "-c".to_string(),
                format!("echo $PORTAL_URL > {}", test_file),
            ];

            let mut child = spawn_child(
                Path::new("/tmp"),
                &args,
                4321,
                "myapp.localhost",
                crate::detect::PortInjection::EnvOnly,
            )
            .await
            .expect("Failed to spawn child");

            let _ = child.wait().await;

            let content = std::fs::read_to_string(&test_file)
                .expect("Failed to read test file");
            assert_eq!(content.trim(), "https://myapp.localhost");

            let _ = std::fs::remove_file(&test_file);
        }

        #[cfg(windows)]
        {
            use rand::Rng;
            let mut rng = rand::thread_rng();
            let random_id = rng.gen::<u32>();
            let test_file = format!("C:\\temp\\portal_url_test_{}.txt", random_id);

            let args = vec![
                "cmd".to_string(),
                "/C".to_string(),
                format!("echo %PORTAL_URL% > {}", test_file),
            ];

            let mut child = spawn_child(Path::new("C:\\"), &args, 4321, "myapp.localhost",
                crate::detect::PortInjection::EnvOnly)
                .await
                .expect("Failed to spawn child");

            let _ = child.wait().await;

            if let Ok(content) = std::fs::read_to_string(&test_file) {
                let url_str = content.trim();
                assert_eq!(url_str, "https://myapp.localhost");
            } else {
                panic!("Failed to read test file");
            }

            let _ = std::fs::remove_file(&test_file);
        }
    }

    #[tokio::test]
    async fn spawn_child_env_only_sets_port_env() {
        #[cfg(unix)]
        {
            use rand::Rng;
            let random_id = rand::thread_rng().gen::<u32>();
            let test_file = format!("/tmp/portal_port_test_{random_id}.txt");
            let args = vec!["sh".to_string(), "-c".to_string(),
                format!("echo $PORT > {test_file}")];
            let mut child = spawn_child(
                Path::new("/tmp"), &args, 4321, "test.localhost",
                crate::detect::PortInjection::EnvOnly,
            ).await.unwrap();
            let _ = child.wait().await;
            let content = std::fs::read_to_string(&test_file).unwrap();
            assert_eq!(content.trim(), "4321");
            let _ = std::fs::remove_file(&test_file);
        }
    }

    #[tokio::test]
    async fn spawn_child_cli_args_appended() {
        #[cfg(unix)]
        {
            use rand::Rng;
            let random_id = rand::thread_rng().gen::<u32>();
            let test_file = format!("/tmp/portal_args_test_{random_id}.txt");
            let args = vec!["sh".to_string(), "-c".to_string(),
                format!("echo \"$0 $@\" > {test_file}")];
            let injection = crate::detect::PortInjection::CliArgs(
                vec!["--port".to_string(), "4321".to_string()]
            );
            let mut child = spawn_child(
                Path::new("/tmp"), &args, 4321, "test.localhost", injection,
            ).await.unwrap();
            let _ = child.wait().await;
            let content = std::fs::read_to_string(&test_file).unwrap();
            assert!(content.contains("--port"), "expected --port in '{content}'");
            let _ = std::fs::remove_file(&test_file);
        }
    }

    #[tokio::test]
    async fn spawn_child_append_address_appended() {
        #[cfg(unix)]
        {
            use rand::Rng;
            let random_id = rand::thread_rng().gen::<u32>();
            let test_file = format!("/tmp/portal_addr_test_{random_id}.txt");
            let args = vec![
                "sh".to_string(), "-c".to_string(),
                format!("echo \"$1\" > {test_file}"),
                "sh".to_string(),
            ];
            let injection = crate::detect::PortInjection::AppendAddress("0.0.0.0:4321".to_string());
            let mut child = spawn_child(
                Path::new("/tmp"), &args, 4321, "test.localhost", injection,
            ).await.unwrap();
            let _ = child.wait().await;
            let content = std::fs::read_to_string(&test_file).unwrap();
            assert!(content.contains("0.0.0.0:4321"), "expected address in '{content}'");
            let _ = std::fs::remove_file(&test_file);
        }
    }
}
