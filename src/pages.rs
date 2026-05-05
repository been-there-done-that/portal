/// HTML-escape a string to prevent injection.
fn html_escape(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '&' => "&amp;".to_string(),
            '<' => "&lt;".to_string(),
            '>' => "&gt;".to_string(),
            '"' => "&quot;".to_string(),
            '\'' => "&#39;".to_string(),
            _ => c.to_string(),
        })
        .collect()
}

/// Generate a 404 Not Found error page.
pub fn page_404(hostname: &str) -> String {
    let escaped_hostname = html_escape(hostname);
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>404 Not Found</title>
    <style>
        body {{
            font-family: system-ui, -apple-system, sans-serif;
            display: flex;
            justify-content: center;
            align-items: center;
            height: 100vh;
            margin: 0;
            background: #f5f5f5;
        }}
        .container {{
            text-align: center;
            background: white;
            padding: 2rem;
            border-radius: 8px;
            box-shadow: 0 2px 8px rgba(0, 0, 0, 0.1);
            max-width: 600px;
        }}
        h1 {{
            font-size: 3rem;
            margin: 0 0 0.5rem 0;
            color: #333;
        }}
        p {{
            margin: 0.5rem 0;
            color: #666;
            font-size: 1.1rem;
        }}
        .hostname {{
            font-family: monospace;
            background: #f0f0f0;
            padding: 0.25rem 0.5rem;
            border-radius: 4px;
            margin: 1rem 0;
        }}
    </style>
</head>
<body>
    <div class="container">
        <h1>404</h1>
        <p>Hostname not found</p>
        <p class="hostname">{}</p>
        <p>The requested hostname is not registered with portal.</p>
    </div>
</body>
</html>"#,
        escaped_hostname
    )
}

/// Generate a 502 Bad Gateway error page.
pub fn page_502(hostname: &str) -> String {
    let escaped_hostname = html_escape(hostname);
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>502 Bad Gateway</title>
    <style>
        body {{
            font-family: system-ui, -apple-system, sans-serif;
            display: flex;
            justify-content: center;
            align-items: center;
            height: 100vh;
            margin: 0;
            background: #f5f5f5;
        }}
        .container {{
            text-align: center;
            background: white;
            padding: 2rem;
            border-radius: 8px;
            box-shadow: 0 2px 8px rgba(0, 0, 0, 0.1);
            max-width: 600px;
        }}
        h1 {{
            font-size: 3rem;
            margin: 0 0 0.5rem 0;
            color: #d32f2f;
        }}
        p {{
            margin: 0.5rem 0;
            color: #666;
            font-size: 1.1rem;
        }}
        .hostname {{
            font-family: monospace;
            background: #f0f0f0;
            padding: 0.25rem 0.5rem;
            border-radius: 4px;
            margin: 1rem 0;
        }}
    </style>
</head>
<body>
    <div class="container">
        <h1>502</h1>
        <p>Bad Gateway</p>
        <p class="hostname">{}</p>
        <p>The upstream server is not responding or unreachable.</p>
    </div>
</body>
</html>"#,
        escaped_hostname
    )
}

/// Generate a 508 Loop Detected error page.
pub fn page_508(hostname: &str) -> String {
    let escaped_hostname = html_escape(hostname);
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>508 Loop Detected</title>
    <style>
        body {{
            font-family: system-ui, -apple-system, sans-serif;
            display: flex;
            justify-content: center;
            align-items: center;
            height: 100vh;
            margin: 0;
            background: #f5f5f5;
        }}
        .container {{
            text-align: center;
            background: white;
            padding: 2rem;
            border-radius: 8px;
            box-shadow: 0 2px 8px rgba(0, 0, 0, 0.1);
            max-width: 600px;
        }}
        h1 {{
            font-size: 3rem;
            margin: 0 0 0.5rem 0;
            color: #f57c00;
        }}
        p {{
            margin: 0.5rem 0;
            color: #666;
            font-size: 1.1rem;
        }}
        .hostname {{
            font-family: monospace;
            background: #f0f0f0;
            padding: 0.25rem 0.5rem;
            border-radius: 4px;
            margin: 1rem 0;
        }}
    </style>
</head>
<body>
    <div class="container">
        <h1>508</h1>
        <p>Loop Detected</p>
        <p class="hostname">{}</p>
        <p>A redirect loop was detected. Check your application configuration.</p>
    </div>
</body>
</html>"#,
        escaped_hostname
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_contain_status_codes() {
        let page_404 = page_404("test.localhost");
        let page_502 = page_502("test.localhost");
        let page_508 = page_508("test.localhost");

        assert!(page_404.contains("404"));
        assert!(page_502.contains("502"));
        assert!(page_508.contains("508"));
    }

    #[test]
    fn pages_contain_hostname() {
        let hostname = "myapp.localhost";
        let page_404 = page_404(hostname);

        assert!(page_404.contains(hostname));
    }

    #[test]
    fn pages_are_valid_html() {
        let page_404 = page_404("test.localhost");
        let page_502 = page_502("test.localhost");
        let page_508 = page_508("test.localhost");

        assert!(page_404.starts_with("<!DOCTYPE html>"));
        assert!(page_404.ends_with("</html>"));

        assert!(page_502.starts_with("<!DOCTYPE html>"));
        assert!(page_502.ends_with("</html>"));

        assert!(page_508.starts_with("<!DOCTYPE html>"));
        assert!(page_508.ends_with("</html>"));
    }

    #[test]
    fn html_escaping_prevents_injection() {
        let malicious = r#"<script>alert('xss')</script>"#;
        let page = page_404(malicious);

        // Should contain escaped version, not raw script
        assert!(!page.contains("<script>"));
        assert!(page.contains("&lt;script&gt;"));
    }
}
