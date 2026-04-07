use crate::proto::Response;
use console::style;

/// Print a generic response. If not ok, print error to stderr and exit(1).
pub fn print_response(resp: &Response) {
    if !resp.ok {
        let msg = resp.error.as_deref().unwrap_or("unknown error");
        eprintln!("error: {msg}");
        std::process::exit(1);
    }
}

/// Print the list of routes as a colored table.
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
        println!("{}", style("No active routes.").dim());
        return;
    }

    println!(
        "{:<30} {:>6}  {}",
        style("HOSTNAME").dim(),
        style("PORT").dim(),
        style("URL").dim()
    );
    println!("{}", style("─".repeat(60)).dim());
    for route in &routes {
        let hostname = route["hostname"].as_str().unwrap_or("-");
        let port = route["port"].as_u64().unwrap_or(0);
        let url = format!("https://{hostname}");
        println!(
            "{:<30} {}  {}",
            style(hostname).dim(),
            style(format!("{port:>6}")).red(),
            style(url).bold().white()
        );
    }
}

/// Print daemon status information with colors.
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

        println!(
            "  {}  {}",
            style(" portal ").bold().white().on_blue(),
            style(format!("v{version}")).dim()
        );
        println!("  {}  {}", style("pid:    ").dim(), style(pid.to_string()).dim());
        println!("  {}  {}s", style("uptime: ").dim(), style(uptime.to_string()).dim());
        println!("  {}  {}", style("routes: ").dim(), style(routes.to_string()).green());
    }
}
