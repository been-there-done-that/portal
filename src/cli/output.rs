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

fn format_uptime(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h}h {m}m {s}s")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}

/// Print a generic response. If not ok, print error to stderr and exit(1).
pub fn print_response(resp: &Response) {
    if !resp.ok {
        let msg = resp.error.as_deref().unwrap_or("unknown error");
        eprintln!("error: {msg}");
        std::process::exit(1);
    }
}

/// Shared routes table renderer used by both print_ls and print_status.
fn print_routes_table(resp: &Response) {
    let routes = match &resp.data {
        Some(serde_json::Value::Array(arr)) if !arr.is_empty() => arr.clone(),
        _ => return,
    };

    let hostname_col = style("HOSTNAME").dim().to_string();
    let port_col = style("PORT").dim().to_string();
    let url_col = style("URL").dim().to_string();
    println!(
        "  {}  {}  {}",
        pad_right(&hostname_col, 30),
        pad_left(&port_col, 6),
        url_col
    );
    println!("  {}", style("─".repeat(58)).dim());
    for route in &routes {
        let hostname = route["hostname"].as_str().unwrap_or("-");
        let port = route["port"].as_u64().unwrap_or(0);
        let url = format!("https://{hostname}");
        let hostname_styled = style(hostname).dim().to_string();
        let port_styled = style(format!("{port}")).red().to_string();
        let url_styled = style(url).bold().white().to_string();
        println!(
            "  {}  {}  {}",
            pad_right(&hostname_styled, 30),
            pad_left(&port_styled, 6),
            url_styled
        );
    }
}

/// Print the list of routes as a colored table.
pub fn print_ls(resp: &Response) {
    if !resp.ok {
        let msg = resp.error.as_deref().unwrap_or("unknown error");
        eprintln!("error: {msg}");
        std::process::exit(1);
    }

    match &resp.data {
        Some(serde_json::Value::Array(arr)) if !arr.is_empty() => {
            print_routes_table(resp);
        }
        _ => {
            println!("{}", style("No active routes.").dim());
        }
    }
}

/// Print daemon status + active routes.
pub fn print_status(status: &Response, routes: &Response) {
    if !status.ok {
        let msg = status.error.as_deref().unwrap_or("unknown error");
        eprintln!("error: {msg}");
        std::process::exit(1);
    }

    if let Some(data) = &status.data {
        let version = data["version"].as_str().unwrap_or("?");
        let pid = data["pid"].as_u64().unwrap_or(0);
        let uptime = format_uptime(data["uptime_secs"].as_u64().unwrap_or(0));
        let http_port = data["http_port"].as_u64().unwrap_or(80);
        let https_port = data["https_port"].as_u64().unwrap_or(443);
        let routes_count = data["routes_count"].as_u64().unwrap_or(0);

        println!(
            "  {}  {}",
            style(" portal ").bold().white().on_blue(),
            style(format!("v{version}")).dim()
        );
        println!();

        let label_w = 10;
        println!(
            "  {}  {}",
            pad_right(&style("pid").dim().to_string(), label_w),
            style(pid.to_string()).dim()
        );
        println!(
            "  {}  {}",
            pad_right(&style("uptime").dim().to_string(), label_w),
            style(&uptime).dim()
        );
        println!(
            "  {}  {}  →  {}",
            pad_right(&style("ports").dim().to_string(), label_w),
            style(format!(":{http_port}")).dim(),
            style(format!(":{https_port}")).dim()
        );
        println!(
            "  {}  {}",
            pad_right(&style("routes").dim().to_string(), label_w),
            style(routes_count.to_string()).green()
        );

        if routes_count > 0 {
            println!();
            print_routes_table(routes);
        }
    } else {
        println!("{}", style("daemon running, no status data available").dim());
    }
}
