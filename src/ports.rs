use rand::seq::SliceRandom;
use std::net::TcpListener;
use crate::error::Result;

pub const BLOCKED_PORTS: &[u16] = &[
    1, 7, 9, 11, 13, 15, 17, 19, 20, 21, 22, 23, 25, 37, 42,
    43, 53, 69, 77, 79, 87, 95, 101, 102, 103, 104, 109, 110, 111, 113,
    115, 117, 119, 123, 135, 137, 139, 143, 161, 179, 389, 427, 465, 512, 513,
    514, 515, 526, 530, 531, 532, 540, 548, 554, 556, 563, 587, 601, 636, 989,
    990, 993, 995, 1719, 1720, 1723, 2049, 3659, 4045, 5060, 5061, 6000, 6566,
    6665, 6666, 6667, 6668, 6669, 6697, 10080,
];

/// Check if a port is blocked by browsers or reserved by the system.
pub fn is_browser_blocked(port: u16) -> bool {
    BLOCKED_PORTS.contains(&port)
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
}
