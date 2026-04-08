// src/hosts.rs

pub(crate) const MARKER_START: &str = "# portless-start";
pub(crate) const MARKER_END: &str = "# portless-end";

/// Returns the path to the system hosts file.
pub fn hosts_path() -> std::path::PathBuf {
    #[cfg(windows)]
    {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
        std::path::PathBuf::from(root)
            .join("System32")
            .join("drivers")
            .join("etc")
            .join("hosts")
    }
    #[cfg(not(windows))]
    {
        std::path::PathBuf::from("/etc/hosts")
    }
}

/// Returns false only when PORTAL_SYNC_HOSTS is "0", "false", "no", or "off". True otherwise.
pub fn should_sync() -> bool {
    !matches!(
        std::env::var("PORTAL_SYNC_HOSTS").as_deref(),
        Ok("0") | Ok("false") | Ok("no") | Ok("off")
    )
}

/// Build the portless-managed block for the given hostnames.
/// Returns an empty string when hostnames is empty.
pub fn build_block(hostnames: &[&str]) -> String {
    if hostnames.is_empty() {
        return String::new();
    }
    let entries = hostnames
        .iter()
        .map(|h| format!("127.0.0.1 {h}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{MARKER_START}\n{entries}\n{MARKER_END}")
}

/// Strip the portless-managed block from hosts file content.
/// Collapses 3+ consecutive blank lines to 2, trims trailing whitespace,
/// and ensures a single trailing newline.
pub fn remove_block(content: &str) -> String {
    let start_idx = content.find(MARKER_START);
    let end_idx = content.find(MARKER_END);
    let (s, e) = match (start_idx, end_idx) {
        (Some(s), Some(e)) if e > s => (s, e),
        _ => return content.to_string(),
    };
    let before = &content[..s];
    let after = &content[e + MARKER_END.len()..];
    let combined = format!("{before}{after}");
    // Collapse 3+ consecutive blank lines to at most 1 blank line (2 newlines)
    let mut out = String::new();
    let mut blank_run = 0usize;
    for line in combined.lines() {
        if line.trim().is_empty() {
            blank_run += 1;
            if blank_run < 2 {
                out.push('\n');
            }
        } else {
            blank_run = 0;
            out.push_str(line);
            out.push('\n');
        }
    }
    let trimmed = out.trim_end();
    if trimmed.is_empty() {
        return String::new();
    }
    format!("{trimmed}\n")
}

/// Extract lines from within the portless-managed block.
/// Inner lines have leading/trailing whitespace trimmed.
/// Returns empty vec if no managed block exists.
pub fn extract_managed(content: &str) -> Vec<String> {
    let start_idx = content.find(MARKER_START);
    let end_idx = content.find(MARKER_END);
    let (s, e) = match (start_idx, end_idx) {
        (Some(s), Some(e)) if e > s => (s, e),
        _ => return vec![],
    };
    content[s + MARKER_START.len()..e]
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_path_is_not_empty() {
        assert!(!hosts_path().as_os_str().is_empty());
    }

    #[cfg(not(windows))]
    #[test]
    fn hosts_path_unix() {
        assert_eq!(hosts_path(), std::path::PathBuf::from("/etc/hosts"));
    }

    #[test]
    fn should_sync_returns_false_for_zero() {
        let original = std::env::var("PORTAL_SYNC_HOSTS").ok();
        unsafe { std::env::set_var("PORTAL_SYNC_HOSTS", "0"); }
        assert!(!should_sync());
        match original {
            Some(val) => unsafe { std::env::set_var("PORTAL_SYNC_HOSTS", val); },
            None => unsafe { std::env::remove_var("PORTAL_SYNC_HOSTS"); },
        }
    }

    #[test]
    fn should_sync_returns_false_for_false() {
        let original = std::env::var("PORTAL_SYNC_HOSTS").ok();
        unsafe { std::env::set_var("PORTAL_SYNC_HOSTS", "false"); }
        assert!(!should_sync());
        match original {
            Some(val) => unsafe { std::env::set_var("PORTAL_SYNC_HOSTS", val); },
            None => unsafe { std::env::remove_var("PORTAL_SYNC_HOSTS"); },
        }
    }

    #[test]
    fn should_sync_returns_false_for_no() {
        let original = std::env::var("PORTAL_SYNC_HOSTS").ok();
        unsafe { std::env::set_var("PORTAL_SYNC_HOSTS", "no"); }
        assert!(!should_sync());
        match original {
            Some(val) => unsafe { std::env::set_var("PORTAL_SYNC_HOSTS", val); },
            None => unsafe { std::env::remove_var("PORTAL_SYNC_HOSTS"); },
        }
    }

    #[test]
    fn should_sync_returns_false_for_off() {
        let original = std::env::var("PORTAL_SYNC_HOSTS").ok();
        unsafe { std::env::set_var("PORTAL_SYNC_HOSTS", "off"); }
        assert!(!should_sync());
        match original {
            Some(val) => unsafe { std::env::set_var("PORTAL_SYNC_HOSTS", val); },
            None => unsafe { std::env::remove_var("PORTAL_SYNC_HOSTS"); },
        }
    }

    #[test]
    fn should_sync_returns_true_for_unset() {
        let original = std::env::var("PORTAL_SYNC_HOSTS").ok();
        unsafe { std::env::remove_var("PORTAL_SYNC_HOSTS"); }
        assert!(should_sync());
        match original {
            Some(val) => unsafe { std::env::set_var("PORTAL_SYNC_HOSTS", val); },
            None => {},
        }
    }

    #[test]
    fn build_block_empty() {
        assert_eq!(build_block(&[]), "");
    }

    #[test]
    fn build_block_single() {
        let block = build_block(&["myapp.localhost"]);
        assert!(block.starts_with("# portless-start\n"));
        assert!(block.contains("127.0.0.1 myapp.localhost"));
        assert!(block.ends_with("\n# portless-end"));
    }

    #[test]
    fn build_block_multiple() {
        let block = build_block(&["myapp.localhost", "api.localhost"]);
        assert!(block.contains("127.0.0.1 myapp.localhost\n127.0.0.1 api.localhost"));
    }

    #[test]
    fn remove_block_no_markers() {
        let content = "127.0.0.1 localhost\n";
        assert_eq!(remove_block(content), content);
    }

    #[test]
    fn remove_block_strips_managed_block() {
        let content = "127.0.0.1 localhost\n\n# portless-start\n127.0.0.1 myapp.localhost\n# portless-end\n";
        let result = remove_block(content);
        assert!(!result.contains("portless-start"));
        assert!(!result.contains("myapp.localhost"));
        assert!(result.contains("127.0.0.1 localhost"));
    }

    #[test]
    fn remove_block_normalises_blank_lines() {
        let content = "a\n\n\n\n# portless-start\nentry\n# portless-end\n";
        let result = remove_block(content);
        assert!(!result.contains("\n\n\n"));
    }

    #[test]
    fn extract_managed_no_block() {
        assert_eq!(extract_managed("127.0.0.1 localhost\n"), vec![] as Vec<String>);
    }

    #[test]
    fn extract_managed_returns_inner_lines() {
        let block = build_block(&["myapp.localhost", "api.localhost"]);
        let lines = extract_managed(&block);
        assert_eq!(lines, vec!["127.0.0.1 myapp.localhost", "127.0.0.1 api.localhost"]);
    }

    #[test]
    fn round_trip_build_extract() {
        let hostnames = &["myapp.localhost", "api.localhost", "admin.local"];
        let block = build_block(hostnames);
        let content = format!("127.0.0.1 localhost\n\n{block}\n");
        let extracted = extract_managed(&content);
        let recovered: Vec<&str> = extracted
            .iter()
            .map(|l| l.splitn(2, ' ').nth(1).unwrap_or(""))
            .collect();
        assert_eq!(recovered, hostnames.to_vec());
    }

    #[test]
    fn remove_then_rebuild_is_idempotent() {
        let hostnames = &["myapp.localhost"];
        let original = "127.0.0.1 localhost\n";
        let with_block = format!("{original}\n{}\n", build_block(hostnames));
        let cleaned = remove_block(&with_block);
        let rebuilt = format!("{}\n{}\n", cleaned.trim_end(), build_block(hostnames));
        let cleaned2 = remove_block(&rebuilt);
        assert_eq!(cleaned, cleaned2);
    }
}
