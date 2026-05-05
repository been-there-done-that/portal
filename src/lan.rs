use std::net::IpAddr;

/// Detect the active LAN IP using a UDP trick: connect to an external address
/// (no packet sent) and read which local interface was chosen.
pub fn detect_lan_ip() -> Option<IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    let addr = socket.local_addr().ok()?;
    let ip = addr.ip();
    if ip.is_loopback() {
        None
    } else {
        Some(ip)
    }
}

/// Spawn a background mDNS publisher for `hostname.local` → `ip`.
/// Returns the child process handle so the caller can kill it on shutdown.
pub fn publish_mdns(
    hostname: &str,
    ip: IpAddr,
    port: u16,
) -> Option<std::process::Child> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("dns-sd")
            .args([
                "-P",
                hostname,
                "_http._tcp",
                "local",
                &port.to_string(),
                &format!("{hostname}.local"),
                &ip.to_string(),
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("avahi-publish-address")
            .args(["-R", &format!("{hostname}.local"), &ip.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (hostname, ip, port);
        None
    }
}

/// Kill a previously started mDNS publisher.
pub fn unpublish_mdns(child: &mut std::process::Child) {
    let _ = child.kill();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_lan_ip_returns_non_loopback_or_none() {
        match detect_lan_ip() {
            Some(ip) => assert!(!ip.is_loopback(), "LAN IP must not be loopback: {ip}"),
            None => {} // acceptable on CI / no active interface
        }
    }

    #[test]
    fn publish_mdns_does_not_panic_without_tools() {
        // Should return None gracefully if dns-sd/avahi-publish-address is absent
        let _ = publish_mdns("test", "192.168.1.1".parse().unwrap(), 80);
    }
}
