use crate::proto::Response;
use console::style;

fn pad_right(s: &str, width: usize) -> String {
    let visual = console::measure_text_width(s);
    let padding = width.saturating_sub(visual);
    format!("{}{}", s, " ".repeat(padding))
}

fn pad_left(s: &str, width: usize) -> String {
    let visual = console::measure_text_width(s);
    let padding = width.saturating_sub(visual);
    format!("{}{}", " ".repeat(padding), s)
}

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
            println!("{}", style("No routes.").dim());
            return;
        }
    };

    if routes.is_empty() {
        println!("{}", style("No active routes.").dim());
        return;
    }

    let hostname_col = style("HOSTNAME").dim().to_string();
    let port_col = style("PORT").dim().to_string();
    let url_col = style("URL").dim().to_string();
    println!(
        "{}  {}  {}",
        pad_right(&hostname_col, 30),
        pad_left(&port_col, 6),
        url_col
    );
    println!("{}", style("─".repeat(60)).dim());
    for route in &routes {
        let hostname = route["hostname"].as_str().unwrap_or("-");
        let port = route["port"].as_u64().unwrap_or(0);
        let url = format!("https://{hostname}");
        let hostname_styled = style(hostname).dim().to_string();
        let port_styled = style(format!("{port}")).red().to_string();
        let url_styled = style(url).bold().white().to_string();
        println!(
            "{}  {}  {}",
            pad_right(&hostname_styled, 30),
            pad_left(&port_styled, 6),
            url_styled
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
        let label_w = 8; // "uptime: " is 8 chars — all labels padded to match
        println!(
            "  {}  {}",
            pad_right(&style("pid:").dim().to_string(), label_w),
            style(pid.to_string()).dim()
        );
        println!(
            "  {}  {}s",
            pad_right(&style("uptime:").dim().to_string(), label_w),
            style(uptime.to_string()).dim()
        );
        println!(
            "  {}  {}",
            pad_right(&style("routes:").dim().to_string(), label_w),
            style(routes.to_string()).green()
        );
    } else {
        println!("{}", style("daemon running, no status data available").dim());
    }
}
