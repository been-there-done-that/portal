use crate::error::{Error, Result};
use std::process::Command;

const SERVE_PORTS: &[u16] = &[443, 8443, 8444, 8445, 8446, 8447, 8448, 8449, 8450, 10000];
const FUNNEL_PORTS: &[u16] = &[443, 8443, 10000];

/// Returns true if the `tailscale` CLI is present and exits with status 0.
pub fn is_installed() -> bool {
    Command::new("tailscale")
        .arg("version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Returns the Tailscale node DNS name, e.g. `myhost.tail12345.ts.net`.
/// Reads `tailscale status --json` and extracts `Self.DNSName`.
pub fn get_node_name() -> Result<String> {
    let out = Command::new("tailscale")
        .args(["status", "--json"])
        .output()
        .map_err(|e| Error::Other(format!("tailscale status failed: {e}")))?;
    if !out.status.success() {
        return Err(Error::Other("tailscale not connected".to_string()));
    }
    let val: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    let name = val["Self"]["DNSName"]
        .as_str()
        .ok_or_else(|| Error::Other("missing DNSName in tailscale status".to_string()))?
        .trim_end_matches('.')
        .to_string();
    Ok(name)
}

/// Returns the list of TCP ports currently configured in `tailscale serve status`.
pub fn used_ports() -> Vec<u16> {
    let Ok(out) = Command::new("tailscale")
        .args(["serve", "status", "--json"])
        .output()
    else {
        return vec![];
    };
    let Ok(val) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else {
        return vec![];
    };
    val["TCP"]
        .as_object()
        .map(|m| {
            m.keys()
                .filter_map(|k| k.trim_start_matches(':').parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

fn find_free_port(funnel: bool) -> Option<u16> {
    let in_use = used_ports();
    let candidates = if funnel { FUNNEL_PORTS } else { SERVE_PORTS };
    candidates.iter().copied().find(|p| !in_use.contains(p))
}

/// Register a local port with Tailscale Serve or Funnel.
///
/// Returns `(https_port, public_url)` on success.
pub fn register(local_port: u16, funnel: bool) -> Result<(u16, String)> {
    let https_port = find_free_port(funnel)
        .ok_or_else(|| Error::Other("no available Tailscale port".to_string()))?;

    let subcmd = if funnel { "funnel" } else { "serve" };
    let out = Command::new("tailscale")
        .args([
            subcmd,
            "--bg",
            "--yes",
            &format!("--https={https_port}"),
            &format!("http://127.0.0.1:{local_port}"),
        ])
        .output()
        .map_err(|e| Error::Other(format!("tailscale {subcmd} failed: {e}")))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if stderr.contains("Funnel not available") || stderr.contains("funnel") {
            return Err(Error::Other(
                "Tailscale Funnel is not enabled on this tailnet. Run: tailscale funnel on"
                    .to_string(),
            ));
        }
        return Err(Error::Other(format!("tailscale {subcmd} failed: {stderr}")));
    }

    let node = get_node_name()?;
    let url = if https_port == 443 {
        format!("https://{node}")
    } else {
        format!("https://{node}:{https_port}")
    };

    Ok((https_port, url))
}

/// Remove a Tailscale Serve or Funnel mapping.
pub fn unregister(https_port: u16, funnel: bool) -> Result<()> {
    let subcmd = if funnel { "funnel" } else { "serve" };
    Command::new("tailscale")
        .args([
            subcmd,
            "--yes",
            &format!("--https={https_port}"),
            "off",
        ])
        .output()
        .map_err(|e| Error::Other(format!("tailscale {subcmd} off failed: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_free_port_avoids_in_use() {
        let candidates = &[443u16, 8443, 8444];
        let in_use = vec![443u16];
        let result = candidates.iter().copied().find(|p| !in_use.contains(p));
        assert_eq!(result, Some(8443));
    }

    #[test]
    fn find_free_port_returns_none_when_all_used() {
        let candidates = &[443u16, 8443];
        let in_use = vec![443u16, 8443];
        let result = candidates.iter().copied().find(|p| !in_use.contains(p));
        assert_eq!(result, None);
    }

    #[test]
    fn funnel_port_candidates_are_subset() {
        assert_eq!(FUNNEL_PORTS, &[443, 8443, 10000]);
    }

    #[test]
    fn serve_port_candidates_include_funnel_ports() {
        for &p in FUNNEL_PORTS {
            assert!(SERVE_PORTS.contains(&p), "SERVE_PORTS must include funnel port {p}");
        }
    }

    #[test]
    fn is_installed_does_not_panic() {
        let _ = is_installed();
    }
}
