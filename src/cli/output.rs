use crate::proto::Response;

/// Print a generic response. If not ok, print error to stderr and exit(1).
pub fn print_response(resp: &Response) {
    if !resp.ok {
        let msg = resp
            .error
            .as_deref()
            .unwrap_or("unknown error");
        eprintln!("error: {msg}");
        std::process::exit(1);
    }
}

/// Print the list of routes as a table.
pub fn print_ls(resp: &Response) {
    if !resp.ok {
        let msg = resp.error.as_deref().unwrap_or("unknown error");
        eprintln!("error: {msg}");
        std::process::exit(1);
    }

    let routes = match &resp.data {
        Some(serde_json::Value::Array(arr)) => arr.clone(),
        _ => {
            println!("No routes.");
            return;
        }
    };

    if routes.is_empty() {
        println!("No active routes.");
        return;
    }

    println!("{:<30} {:>6}  {}", "HOSTNAME", "PORT", "URL");
    println!("{}", "-".repeat(60));
    for route in &routes {
        let hostname = route["hostname"].as_str().unwrap_or("-");
        let port = route["port"].as_u64().unwrap_or(0);
        let url = format!("https://{hostname}");
        println!("{:<30} {:>6}  {}", hostname, port, url);
    }
}

/// Print daemon status information.
pub fn print_status(resp: &Response) {
    if !resp.ok {
        let msg = resp.error.as_deref().unwrap_or("unknown error");
        eprintln!("error: {msg}");
        std::process::exit(1);
    }

    if let Some(data) = &resp.data {
        let version = data["version"].as_str().unwrap_or("?");
        let pid = data["pid"].as_u64().unwrap_or(0);
        let uptime = data["uptime_secs"].as_u64().unwrap_or(0);
        let routes = data["routes_count"].as_u64().unwrap_or(0);

        println!("portless v{version}");
        println!("  pid:     {pid}");
        println!("  uptime:  {uptime}s");
        println!("  routes:  {routes}");
    }
}
