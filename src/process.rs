use crate::error::Result;
use std::path::Path;

pub const PORTLESS_URL_ENV: &str = "PORTLESS_URL";

/// Spawn a child dev server process.
/// Callers provide `extra_env` — all env vars to set (PORT, PORTAL_URL, NODE_EXTRA_CA_CERTS, etc.).
/// Handles PortInjection variants for framework-specific port passing.
pub async fn spawn_child(
    cwd: &Path,
    args: &[String],
    port: u16,
    injection: crate::detect::PortInjection,
    extra_env: &[(String, String)],
) -> Result<tokio::process::Child> {
    if args.is_empty() {
        return Err(crate::error::Error::Ipc(
            "No arguments provided to spawn_child".to_string(),
        ));
    }

    let port_str = port.to_string();

    // Substitute {port} in every arg
    let args: Vec<String> = args
        .iter()
        .map(|a| a.replace("{port}", &port_str))
        .collect();

    let program = &args[0];
    let rest: Vec<&str> = args[1..].iter().map(String::as_str).collect();

    let mut cmd = tokio::process::Command::new(program);
    #[cfg(unix)]
    cmd.process_group(0);
    cmd.current_dir(cwd).kill_on_drop(false);

    for (k, v) in extra_env {
        cmd.env(k, v);
    }

    match injection {
        crate::detect::PortInjection::EnvOnly => {
            cmd.args(&rest);
        }
        crate::detect::PortInjection::CliArgs(ref extra) => {
            cmd.args(&rest);
            if !extra.is_empty() && needs_double_dash_separator(&args) {
                cmd.arg("--");
            }
            cmd.args(extra);
        }
        crate::detect::PortInjection::AppendAddress(ref addr) => {
            cmd.args(&rest).arg(addr);
        }
    }

    Ok(cmd.spawn()?)
}

/// Safely send SIGTERM to a process group. Validates the PID to prevent
/// broadcasting to all processes (which happens if pid wraps to -1 via `as i32`).
#[cfg(unix)]
pub fn safe_killpg_term(pid: u32) {
    use nix::sys::signal::{killpg, Signal};
    use nix::unistd::Pid;
    let raw = match i32::try_from(pid) {
        Ok(v) if v > 0 => v,
        _ => return, // invalid PID — don't signal
    };
    killpg(Pid::from_raw(raw), Signal::SIGTERM).ok();
}

/// Returns true when the command is `npm/yarn/pnpm/bun run <script>`.
/// These package managers treat extra args as their own flags unless preceded by `--`.
fn needs_double_dash_separator(args: &[String]) -> bool {
    if args.len() < 3 {
        return false;
    }
    let runner = args[0].rsplit('/').next().unwrap_or(&args[0]);
    matches!(runner, "npm" | "yarn" | "pnpm" | "bun") && args[1] == "run"
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

    // Send SIGTERM to process group
    #[cfg(unix)]
    safe_killpg_term(pid);

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

const BUILD_ONLY_TOOLS: &[&str] = &[
    "tsc", "tsup", "esbuild", "rollup", "webpack", "parcel",
];

const BUILD_ONLY_SUBCMDS: &[(&str, &str)] = &[
    ("vite", "build"),
    ("next", "build"),
    ("bun", "build"),
    ("turbo", "build"),
    ("nuxt", "build"),
    ("astro", "build"),
    ("svelte-kit", "build"),
    ("rspack", "build"),
    ("rsbuild", "build"),
];

/// Returns true when `args` represents a build-only tool that should not
/// be proxied (it produces artifacts, not a long-running server).
pub fn is_build_only(args: &[String]) -> bool {
    let Some(first) = args.first() else { return false };

    let basename = std::path::Path::new(first.as_str())
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(first.as_str());
    // Strip Windows .cmd / .exe suffixes
    let basename = basename
        .strip_suffix(".cmd")
        .or_else(|| basename.strip_suffix(".exe"))
        .unwrap_or(basename);

    if BUILD_ONLY_TOOLS.contains(&basename) {
        return true;
    }

    if let Some(second) = args.get(1) {
        for (tool, subcmd) in BUILD_ONLY_SUBCMDS {
            if basename == *tool && second == *subcmd {
                return true;
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn spawns_and_kills_child() {
        #[cfg(unix)]
        {
            let args = vec!["sleep".to_string(), "60".to_string()];
            let mut child = spawn_child(
                Path::new("/tmp"),
                &args,
                4321,
                crate::detect::PortInjection::EnvOnly,
                &[],
            )
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
            let mut child = spawn_child(
                Path::new("C:\\"),
                &args,
                4321,
                crate::detect::PortInjection::EnvOnly,
                &[],
            )
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

            let extra_env = vec![("PORT".to_string(), "4321".to_string())];
            let mut child = spawn_child(
                Path::new("/tmp"),
                &args,
                4321,
                crate::detect::PortInjection::EnvOnly,
                &extra_env,
            )
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

            let extra_env = vec![("PORT".to_string(), "4321".to_string())];
            let mut child = spawn_child(
                Path::new("C:\\"),
                &args,
                4321,
                crate::detect::PortInjection::EnvOnly,
                &extra_env,
            )
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
                format!("echo $PORTLESS_URL > {}", test_file),
            ];

            let extra_env = vec![(
                PORTLESS_URL_ENV.to_string(),
                "https://myapp.localhost".to_string(),
            )];
            let mut child = spawn_child(
                Path::new("/tmp"),
                &args,
                4321,
                crate::detect::PortInjection::EnvOnly,
                &extra_env,
            )
            .await
            .expect("Failed to spawn child");

            let _ = child.wait().await;

            let content = std::fs::read_to_string(&test_file).expect("Failed to read test file");
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
                format!("echo %PORTLESS_URL% > {}", test_file),
            ];

            let extra_env = vec![(
                PORTLESS_URL_ENV.to_string(),
                "https://myapp.localhost".to_string(),
            )];
            let mut child = spawn_child(
                Path::new("C:\\"),
                &args,
                4321,
                crate::detect::PortInjection::EnvOnly,
                &extra_env,
            )
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
    async fn spawn_child_cli_args_appended() {
        #[cfg(unix)]
        {
            use rand::Rng;
            let random_id = rand::thread_rng().gen::<u32>();
            let test_file = format!("/tmp/portal_args_test_{random_id}.txt");
            let args = vec![
                "sh".to_string(),
                "-c".to_string(),
                format!("echo \"$0 $@\" > {test_file}"),
            ];
            let injection = crate::detect::PortInjection::CliArgs(vec![
                "--port".to_string(),
                "4321".to_string(),
            ]);
            let mut child = spawn_child(Path::new("/tmp"), &args, 4321, injection, &[])
                .await
                .unwrap();
            let _ = child.wait().await;
            let content = std::fs::read_to_string(&test_file).unwrap();
            assert!(content.contains("--port"), "expected --port in '{content}'");
            let _ = std::fs::remove_file(&test_file);
        }
    }

    #[tokio::test]
    async fn spawn_child_uses_separate_process_group() {
        #[cfg(unix)]
        {
            use rand::Rng;
            let id = rand::thread_rng().gen::<u32>();
            let pgid_file = format!("/tmp/portal_pgid_{id}.txt");

            // sh writes its own process group ID to a file
            let args = vec![
                "sh".to_string(),
                "-c".to_string(),
                format!("ps -o pgid= -p $$ | tr -d ' ' > {pgid_file}"),
            ];
            let mut child = spawn_child(
                Path::new("/tmp"),
                &args,
                4321,
                crate::detect::PortInjection::EnvOnly,
                &[],
            )
            .await
            .unwrap();
            let _ = child.wait().await;

            let child_pgid: u32 = std::fs::read_to_string(&pgid_file)
                .unwrap_or_default()
                .trim()
                .parse()
                .unwrap_or(0);
            let portal_pgid = unsafe { nix::libc::getpgrp() } as u32;

            // Child should be in a different process group than portal
            assert_ne!(child_pgid, 0, "child pgid should be non-zero");
            assert_ne!(
                child_pgid, portal_pgid,
                "child pgid ({child_pgid}) should differ from portal pgid ({portal_pgid})"
            );

            let _ = std::fs::remove_file(&pgid_file);
        }
    }

    #[tokio::test]
    async fn stop_child_kills_entire_process_group() {
        #[cfg(unix)]
        {
            use rand::Rng;
            let id = rand::thread_rng().gen::<u32>();
            let pid_file = format!("/tmp/portal_grandchild_{id}.txt");

            // sh spawns `sleep 300` in the background and writes its PID to a file,
            // then waits — so sh is the direct child and sleep is a grandchild.
            let args = vec![
                "sh".to_string(),
                "-c".to_string(),
                format!("sleep 300 & echo $! > {pid_file}; wait"),
            ];
            let mut child = spawn_child(
                Path::new("/tmp"),
                &args,
                4321,
                crate::detect::PortInjection::EnvOnly,
                &[],
            )
            .await
            .unwrap();

            // Wait for grandchild to start and write its PID
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;

            let grandchild_pid: u32 = std::fs::read_to_string(&pid_file)
                .unwrap_or_default()
                .trim()
                .parse()
                .unwrap_or(0);
            assert!(
                grandchild_pid > 0,
                "grandchild pid should be > 0, got file content: {:?}",
                std::fs::read_to_string(&pid_file)
            );
            assert!(
                crate::routes::pid_alive_check(grandchild_pid),
                "grandchild (sleep) should be alive before stop_child"
            );

            // stop_child should kill the whole process group (sh + sleep)
            let _ = stop_child(&mut child).await;
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;

            assert!(
                !crate::routes::pid_alive_check(grandchild_pid),
                "grandchild (sleep) should be dead after stop_child killed the process group"
            );
            let _ = std::fs::remove_file(&pid_file);
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
                "sh".to_string(),
                "-c".to_string(),
                format!("echo \"$1\" > {test_file}"),
                "sh".to_string(),
            ];
            let injection = crate::detect::PortInjection::AppendAddress("0.0.0.0:4321".to_string());
            let mut child = spawn_child(Path::new("/tmp"), &args, 4321, injection, &[])
                .await
                .unwrap();
            let _ = child.wait().await;
            let content = std::fs::read_to_string(&test_file).unwrap();
            assert!(
                content.contains("0.0.0.0:4321"),
                "expected address in '{content}'"
            );
            let _ = std::fs::remove_file(&test_file);
        }
    }

    #[tokio::test]
    async fn extra_env_vars_are_passed_to_child() {
        #[cfg(unix)]
        {
            use rand::Rng;
            let random_id = rand::thread_rng().gen::<u32>();
            let test_file = format!("/tmp/portal_extra_env_{random_id}.txt");
            let args = vec![
                "sh".to_string(),
                "-c".to_string(),
                format!("echo $MY_CUSTOM_VAR > {test_file}"),
            ];
            let extra_env = vec![("MY_CUSTOM_VAR".to_string(), "hello123".to_string())];
            let mut child = spawn_child(
                Path::new("/tmp"),
                &args,
                4321,
                crate::detect::PortInjection::EnvOnly,
                &extra_env,
            )
            .await
            .expect("spawn failed");
            let _ = child.wait().await;
            let content = std::fs::read_to_string(&test_file).unwrap();
            assert_eq!(content.trim(), "hello123");
            let _ = std::fs::remove_file(&test_file);
        }
    }

    #[tokio::test]
    async fn port_env_not_set_when_not_in_extra_env() {
        #[cfg(unix)]
        {
            use rand::Rng;
            let random_id = rand::thread_rng().gen::<u32>();
            let test_file = format!("/tmp/portal_no_port_{random_id}.txt");
            let args = vec![
                "sh".to_string(),
                "-c".to_string(),
                format!("echo \"$PORT\" > {test_file}"),
            ];
            let mut child = spawn_child(
                Path::new("/tmp"),
                &args,
                4321,
                crate::detect::PortInjection::EnvOnly,
                &[],
            )
            .await
            .expect("spawn failed");
            let _ = child.wait().await;
            let content = std::fs::read_to_string(&test_file).unwrap();
            assert_ne!(
                content.trim(),
                "4321",
                "PORT should not be auto-injected by spawn_child; got: {content}"
            );
            let _ = std::fs::remove_file(&test_file);
        }
    }

    #[test]
    fn double_dash_needed_for_npm_run() {
        let args = vec!["npm".into(), "run".into(), "start".into()];
        assert!(needs_double_dash_separator(&args));
    }

    #[test]
    fn double_dash_needed_for_pnpm_run() {
        let args = vec!["pnpm".into(), "run".into(), "dev".into()];
        assert!(needs_double_dash_separator(&args));
    }

    #[test]
    fn double_dash_needed_for_yarn_run() {
        let args = vec!["yarn".into(), "run".into(), "dev".into()];
        assert!(needs_double_dash_separator(&args));
    }

    #[test]
    fn double_dash_not_needed_for_direct_node() {
        let args = vec!["node".into(), "server.js".into()];
        assert!(!needs_double_dash_separator(&args));
    }

    #[test]
    fn double_dash_not_needed_for_npx() {
        let args = vec!["npx".into(), "vite".into()];
        assert!(!needs_double_dash_separator(&args));
    }

    #[test]
    fn double_dash_not_needed_for_npm_without_run() {
        let args = vec!["npm".into(), "start".into()];
        assert!(!needs_double_dash_separator(&args));
    }

    #[test]
    fn portless_url_env_var_name_is_correct() {
        // Ensures the exported env var name matches the JS reference implementation
        assert_eq!(crate::process::PORTLESS_URL_ENV, "PORTLESS_URL");
    }

    #[test]
    fn tsc_is_build_only() {
        assert!(is_build_only(&["tsc".to_string()]));
    }

    #[test]
    fn tsup_is_build_only() {
        assert!(is_build_only(&["tsup".to_string(), "src/index.ts".to_string()]));
    }

    #[test]
    fn vite_build_is_build_only() {
        assert!(is_build_only(&["vite".to_string(), "build".to_string()]));
    }

    #[test]
    fn vite_dev_is_not_build_only() {
        assert!(!is_build_only(&["vite".to_string()]));
        assert!(!is_build_only(&["vite".to_string(), "dev".to_string()]));
    }

    #[test]
    fn next_build_is_build_only() {
        assert!(is_build_only(&["next".to_string(), "build".to_string()]));
    }

    #[test]
    fn next_dev_is_not_build_only() {
        assert!(!is_build_only(&["next".to_string(), "dev".to_string()]));
    }

    #[test]
    fn node_server_is_not_build_only() {
        assert!(!is_build_only(&["node".to_string(), "server.js".to_string()]));
    }

    #[test]
    fn empty_args_is_not_build_only() {
        assert!(!is_build_only(&[]));
    }
}
