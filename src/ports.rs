use crate::error::Result;
use rand::seq::SliceRandom;
use std::net::TcpListener;

pub const BLOCKED_PORTS: &[u16] = &[
    1, 7, 9, 11, 13, 15, 17, 19, 20, 21, 22, 23, 25, 37, 42, 43, 53, 69, 77, 79, 87, 95, 101, 102,
    103, 104, 109, 110, 111, 113, 115, 117, 119, 123, 135, 137, 139, 143, 161, 179, 389, 427, 465,
    512, 513, 514, 515, 526, 530, 531, 532, 540, 548, 554, 556, 563, 587, 601, 636, 989, 990, 993,
    995, 1719, 1720, 1723, 2049, 3659, 4045, 5060, 5061, 6000, 6566, 6665, 6666, 6667, 6668, 6669,
    6697, 10080,
];

/// Check if a port is blocked by browsers or reserved by the system.
pub fn is_browser_blocked(port: u16) -> bool {
    BLOCKED_PORTS.contains(&port)
}

/// Validate an explicitly provided app port.
/// Returns `Err(InvalidPort)` if the port is privileged (< 1024) or browser-blocked.
pub fn validate_app_port(port: u16) -> Result<()> {
    if port < 1024 {
        return Err(crate::error::Error::InvalidPort(format!(
            "port {port} is a privileged port (< 1024)"
        )));
    }
    if is_browser_blocked(port) {
        return Err(crate::error::Error::InvalidPort(format!(
            "port {port} is blocked by browsers — see https://fetch.spec.whatwg.org/#bad-port"
        )));
    }
    Ok(())
}

/// Find a free port in the range [lo, hi] (inclusive).
/// Skips browser-blocked ports and ports < 1024.
/// Returns Error::NoFreePort if no port is available.
pub fn find_free_port(lo: u16, hi: u16) -> Result<u16> {
    let mut ports: Vec<u16> = (lo..=hi).collect();

    // Shuffle the ports for randomness
    let mut rng = rand::thread_rng();
    ports.shuffle(&mut rng);

    for port in ports {
        // Skip browser-blocked ports
        if is_browser_blocked(port) {
            continue;
        }

        // Skip privileged ports (< 1024)
        if port < 1024 {
            continue;
        }

        // Try to bind to the port
        if let Ok(listener) = TcpListener::bind(format!("127.0.0.1:{}", port)) {
            // Successfully bound — port is free
            drop(listener); // Close the listener immediately
            return Ok(port);
        }
    }

    // No free port found
    Err(crate::error::Error::NoFreePort(lo, hi))
}

pub fn find_free_port_excluding(lo: u16, hi: u16, excluded: &[u16]) -> Result<u16> {
    let mut ports: Vec<u16> = (lo..=hi).collect();

    let mut rng = rand::thread_rng();
    ports.shuffle(&mut rng);

    for port in ports {
        if excluded.contains(&port) || is_browser_blocked(port) || port < 1024 {
            continue;
        }

        if let Ok(listener) = TcpListener::bind(format!("127.0.0.1:{port}")) {
            drop(listener);
            return Ok(port);
        }
    }

    Err(crate::error::Error::NoFreePort(lo, hi))
}

/// Poll until `port` is no longer accepting connections (i.e., the previous
/// process has released it), or until `timeout` elapses.
/// Never returns an error — on timeout it simply returns so the caller can
/// proceed (the new process will bind once the old one fully exits).
pub async fn wait_for_port_free(port: u16, timeout: std::time::Duration) {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        // Port is truly free when we can bind to it (not just when connect fails).
        // Bind 127.0.0.1 to avoid briefly listening on all interfaces.
        // spawn_blocking keeps this blocking syscall off the async executor.
        let free = tokio::task::spawn_blocking(move || {
            std::net::TcpListener::bind(format!("127.0.0.1:{port}")).is_ok()
        })
        .await
        .unwrap_or(false);
        if free {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assigns_a_free_port_in_range() {
        let port = find_free_port(4000, 4999).unwrap();
        assert!(port >= 4000 && port <= 4999);
    }

    #[test]
    fn skips_already_bound_ports() {
        // Bind port 4050 manually
        let listener = TcpListener::bind("127.0.0.1:4050").unwrap();

        // Find a free port in the range that includes 4050
        // This should not return 4050
        let port = find_free_port(4050, 4060).unwrap();
        assert_ne!(port, 4050);
        assert!(port >= 4050 && port <= 4060);

        drop(listener);
    }

    #[test]
    fn errors_when_range_exhausted() {
        // Bind the only port in the range
        let listener = TcpListener::bind("127.0.0.1:4200").unwrap();

        // Try to find a free port in a range with only one port
        let result = find_free_port(4200, 4200);
        assert!(result.is_err());

        drop(listener);
    }

    #[test]
    fn skips_browser_blocked_ports() {
        // Port 6000 should be blocked
        assert!(is_browser_blocked(6000));
        // Port 4000 should not be blocked
        assert!(!is_browser_blocked(4000));
    }

    #[test]
    fn validate_rejects_privileged_port() {
        let err = validate_app_port(80).unwrap_err();
        assert!(err.to_string().contains("privileged"));
    }

    #[test]
    fn validate_rejects_privileged_boundary() {
        let err = validate_app_port(1023).unwrap_err();
        assert!(err.to_string().contains("privileged"));
    }

    #[test]
    fn validate_accepts_port_1024() {
        assert!(validate_app_port(1024).is_ok());
    }

    #[test]
    fn validate_rejects_browser_blocked_port_6000() {
        let err = validate_app_port(6000).unwrap_err();
        assert!(err.to_string().contains("blocked"));
    }

    #[test]
    fn validate_rejects_browser_blocked_irc_ports() {
        for port in [6665u16, 6666, 6667, 6668, 6669] {
            let err = validate_app_port(port).unwrap_err();
            assert!(
                err.to_string().contains("blocked"),
                "port {port} should be blocked"
            );
        }
    }

    #[test]
    fn validate_accepts_normal_port() {
        assert!(validate_app_port(4000).is_ok());
        assert!(validate_app_port(3000).is_ok());
        assert!(validate_app_port(8080).is_ok());
    }

    #[test]
    fn excludes_specific_ports_when_searching() {
        let port = find_free_port_excluding(4300, 4310, &[4305]).unwrap();
        assert_ne!(port, 4305);
    }

    #[tokio::test]
    async fn returns_immediately_when_port_is_free() {
        // Port 19997 should be free; function should return within 200ms
        let start = std::time::Instant::now();
        wait_for_port_free(19997, std::time::Duration::from_secs(2)).await;
        assert!(
            start.elapsed() < std::time::Duration::from_millis(500),
            "should return quickly when port is free, took {:?}",
            start.elapsed()
        );
    }

    #[tokio::test]
    async fn wait_for_port_free_waits_until_port_is_released() {
        // Bind a random port, then release it after 200ms — verify we wait for it.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        // Release the listener after 200ms from a background task
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            drop(listener); // port released here
        });

        let start = std::time::Instant::now();
        wait_for_port_free(port, std::time::Duration::from_secs(2)).await;
        let elapsed = start.elapsed();

        assert!(
            elapsed >= std::time::Duration::from_millis(100),
            "should have waited for port to be released, returned after only {:?}",
            elapsed
        );
        assert!(
            elapsed < std::time::Duration::from_millis(900),
            "should have returned promptly after release, waited {:?}",
            elapsed
        );
    }

    #[tokio::test]
    async fn wait_for_port_free_returns_immediately_when_already_free() {
        // Use a port with nothing bound on it
        let port = find_free_port(19900, 19950).unwrap();
        let start = std::time::Instant::now();
        wait_for_port_free(port, std::time::Duration::from_secs(2)).await;
        assert!(
            start.elapsed() < std::time::Duration::from_millis(200),
            "free port should return immediately"
        );
    }

    #[tokio::test]
    async fn times_out_when_port_stays_bound() {
        use std::net::TcpListener;
        // Bind port 19996 to simulate a still-running process.
        let listener = TcpListener::bind("127.0.0.1:19996").unwrap();
        let start = std::time::Instant::now();
        // Wait with a 350ms timeout
        wait_for_port_free(19996, std::time::Duration::from_millis(350)).await;
        let elapsed = start.elapsed();
        // Should have waited at least ~300ms before timing out
        assert!(
            elapsed >= std::time::Duration::from_millis(250),
            "should have waited for timeout, only waited {:?}",
            elapsed
        );
        drop(listener);
    }
}
