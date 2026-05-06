/// Build the HTML snippet to inject before </body> for the app-switcher UI.
pub fn build_switcher_html(
    hostname: &str,
    slots: &[crate::routes::Route],
    current_slot: u32,
) -> String {
    let cookie_key = format!("portless-slot-{}", hostname.replace('.', "-"));

    let mut buttons_vec = Vec::new();
    for r in slots {
        let label_owned;
        let label: &str = match &r.label {
            Some(s) => s.as_str(),
            None => {
                label_owned = format!("slot-{}", r.slot);
                &label_owned
            }
        };
        let active = if r.slot == current_slot {
            "background:#5c6ac4;"
        } else {
            "background:#333;"
        };
        buttons_vec.push(format!(
            r#"<button data-portless-slot="{slot}" style="border:none;color:#fff;padding:3px 8px;border-radius:4px;cursor:pointer;{active}">{label}</button>"#,
            slot = r.slot,
            label = label,
            active = active,
        ));
    }
    let buttons = buttons_vec.join("\n  ");

    format!(
        r#"
<div id="__portless_switcher__" style="position:fixed;bottom:16px;right:16px;z-index:99999;background:#1a1a1a;color:#fff;border-radius:8px;padding:8px 12px;font:13px/1.4 monospace;box-shadow:0 4px 12px rgba(0,0,0,.4);display:flex;gap:8px;align-items:center">
  <span style="opacity:.5;margin-right:4px">portal</span>
  {buttons}
</div>
<script>
(function(){{
  var key='{cookie_key}';
  document.querySelectorAll('[data-portless-slot]').forEach(function(b){{
    b.addEventListener('click',function(){{
      document.cookie=key+'='+b.dataset.portlessSlot+';path=/;max-age=86400';
      location.reload();
    }});
  }});
}})();
</script>
"#,
        buttons = buttons,
        cookie_key = cookie_key,
    )
}

/// Inject the switcher HTML before </body> in an HTML body.
/// Returns `None` if content_type is not text/html, body > 4MB, or only 1 slot.
pub fn inject_switcher(
    body: &[u8],
    content_type: &str,
    hostname: &str,
    slots: &[crate::routes::Route],
    current_slot: u32,
) -> Option<Vec<u8>> {
    if slots.len() <= 1 {
        return None;
    }
    if !content_type.starts_with("text/html") {
        return None;
    }
    if body.len() > 4 * 1024 * 1024 {
        return None;
    }

    let html = build_switcher_html(hostname, slots, current_slot);
    let body_str = std::str::from_utf8(body).ok()?;

    // Find last </body> (case-insensitive)
    let lower = body_str.to_lowercase();
    if let Some(pos) = lower.rfind("</body>") {
        let mut result = body_str[..pos].as_bytes().to_vec();
        result.extend_from_slice(html.as_bytes());
        result.extend_from_slice(body_str[pos..].as_bytes());
        Some(result)
    } else {
        // No </body> tag — append to end
        let mut result = body.to_vec();
        result.extend_from_slice(html.as_bytes());
        Some(result)
    }
}

/// Parse the preferred slot number from the `Cookie` header value.
/// Cookie key format: `portless-slot-<hostname-with-dots-as-dashes>=<N>`.
pub fn read_slot_from_cookies(cookie_header: &str, hostname: &str) -> u32 {
    let key = format!("portless-slot-{}=", hostname.replace('.', "-"));
    cookie_header
        .split(';')
        .map(|s| s.trim())
        .find(|s| s.starts_with(&key))
        .and_then(|s| s[key.len()..].parse().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::{Route, RouteProtocol};

    fn make_slot(slot: u32, label: Option<&str>) -> Route {
        Route {
            hostname: "myapp.localhost".to_string(),
            port: 4000 + slot as u16,
            public_port: None,
            protocol: RouteProtocol::Http,
            pid: std::process::id(),
            owner_pid: std::process::id(),
            cwd: "/tmp".to_string(),
            created_at: chrono::Utc::now(),
            slot,
            label: label.map(String::from),
            tailscale_url: None,
            tailscale_https_port: None,
            tailscale_funnel: false,
        }
    }

    #[test]
    fn inject_inserts_before_body_close() {
        let slots = vec![make_slot(0, Some("main")), make_slot(1, Some("dev"))];
        let html = b"<html><body><p>hello</p></body></html>";
        let result = inject_switcher(html, "text/html", "myapp.localhost", &slots, 0).unwrap();
        let s = String::from_utf8(result).unwrap();
        assert!(s.contains("__portless_switcher__"));
        assert!(s.contains("</body></html>"));
        assert!(s.rfind("__portless_switcher__").unwrap() < s.rfind("</body>").unwrap());
    }

    #[test]
    fn inject_skips_non_html() {
        let slots = vec![make_slot(0, None), make_slot(1, None)];
        let result = inject_switcher(b"{}", "application/json", "x.localhost", &slots, 0);
        assert!(result.is_none());
    }

    #[test]
    fn inject_skips_single_slot() {
        let slots = vec![make_slot(0, None)];
        let result = inject_switcher(b"<html></html>", "text/html", "x.localhost", &slots, 0);
        assert!(result.is_none());
    }

    #[test]
    fn inject_appends_when_no_body_tag() {
        let slots = vec![make_slot(0, None), make_slot(1, None)];
        let result =
            inject_switcher(b"<p>no body tag</p>", "text/html", "x.localhost", &slots, 0)
                .unwrap();
        let s = String::from_utf8(result).unwrap();
        assert!(s.contains("__portless_switcher__"));
        assert!(s.starts_with("<p>no body tag</p>"));
    }

    #[test]
    fn inject_skips_oversized_body() {
        let slots = vec![make_slot(0, None), make_slot(1, None)];
        let big = vec![b'x'; 5 * 1024 * 1024];
        let result = inject_switcher(&big, "text/html", "x.localhost", &slots, 0);
        assert!(result.is_none());
    }

    #[test]
    fn active_slot_button_has_different_style() {
        let slots = vec![make_slot(0, Some("main")), make_slot(1, Some("dev"))];
        let html = build_switcher_html("myapp.localhost", &slots, 1);
        assert!(html.contains("#5c6ac4"), "active slot should have highlight colour");
        assert!(html.contains("#333"), "inactive slot should have default colour");
    }

    #[test]
    fn read_slot_from_cookies_parses_correctly() {
        let cookie = "other=val; portless-slot-myapp-localhost=2; another=x";
        assert_eq!(read_slot_from_cookies(cookie, "myapp.localhost"), 2);
    }

    #[test]
    fn read_slot_from_cookies_returns_zero_when_absent() {
        assert_eq!(read_slot_from_cookies("other=val", "myapp.localhost"), 0);
    }

    #[test]
    fn read_slot_from_cookies_returns_zero_for_empty_header() {
        assert_eq!(read_slot_from_cookies("", "myapp.localhost"), 0);
    }
}
