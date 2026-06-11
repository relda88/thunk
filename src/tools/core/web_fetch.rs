use std::io::Read;
use std::net::ToSocketAddrs;

use crate::runtime::ResolvedToolInput;
use crate::tools::types::{
    ExecutionKind, ToolError, ToolOutput, ToolRunResult, ToolSpec, WebFetchOutput,
};
use crate::tools::Tool;

const MAX_FETCH_BYTES: u64 = 32 * 1024;

pub struct WebFetchTool {
    enabled: bool,
}

impl WebFetchTool {
    pub fn new(enabled: bool) -> Self {
        Self { enabled }
    }
}

impl Tool for WebFetchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "web_fetch",
            description: "Fetch a URL and return its content as plain text.",
            input_hint: "",
            execution_kind: ExecutionKind::Immediate,
            default_risk: None,
        }
    }

    fn run(&self, input: &ResolvedToolInput) -> Result<ToolRunResult, ToolError> {
        let ResolvedToolInput::WebFetch { url } = input else {
            return Err(ToolError::InvalidInput(
                "web_fetch received wrong input variant".into(),
            ));
        };

        if !self.enabled {
            return Err(ToolError::InvalidInput(
                "web_fetch: disabled in config".into(),
            ));
        }

        validate_url(url)?;

        let (status_code, raw_body) = fetch_url(url)?;

        let is_html = raw_body.trim_start().starts_with('<')
            || raw_body.to_ascii_lowercase().contains("<html");

        let (title, content) = if is_html {
            strip_html(&raw_body)
        } else {
            (String::new(), raw_body.clone())
        };

        let bytes_fetched = raw_body.len().min(MAX_FETCH_BYTES as usize);
        let truncated = raw_body.len() >= MAX_FETCH_BYTES as usize;

        Ok(ToolRunResult::Immediate(ToolOutput::WebFetch(
            WebFetchOutput {
                url: url.clone(),
                title,
                content,
                status_code,
                truncated,
                bytes_fetched,
            },
        )))
    }
}

fn extract_host(url: &str) -> Option<String> {
    let after_scheme = url.split("://").nth(1)?;
    let host_part = after_scheme.split('/').next()?;
    let host = if host_part.contains(':') {
        host_part.split(':').next()?
    } else {
        host_part
    };
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

fn is_private_ip(addr: std::net::IpAddr) -> bool {
    match addr {
        std::net::IpAddr::V4(a) => {
            let o = a.octets();
            o[0] == 127
                || o[0] == 10
                || (o[0] == 172 && (16..=31).contains(&o[1]))
                || (o[0] == 192 && o[1] == 168)
                || (o[0] == 169 && o[1] == 254)
                || o == [0, 0, 0, 0]
        }
        std::net::IpAddr::V6(a) => a.is_loopback() || a.segments()[0] == 0xfe80,
    }
}

fn validate_url(url: &str) -> Result<(), ToolError> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(ToolError::InvalidInput(
            "web_fetch: URL must start with http:// or https://".into(),
        ));
    }
    let host = extract_host(url).ok_or_else(|| {
        ToolError::InvalidInput("web_fetch: could not parse host from URL".into())
    })?;
    let addrs = (host.as_str(), 80_u16)
        .to_socket_addrs()
        .map_err(|e| ToolError::InvalidInput(format!("web_fetch: DNS resolution failed: {e}")))?;
    for addr in addrs {
        if is_private_ip(addr.ip()) {
            return Err(ToolError::InvalidInput(
                "web_fetch: private/loopback addresses not permitted".into(),
            ));
        }
    }
    Ok(())
}

fn fetch_url(url: &str) -> Result<(u16, String), ToolError> {
    let url = url.to_string();
    let handle = std::thread::spawn(move || {
        ureq::get(&url)
            .timeout(std::time::Duration::from_secs(10))
            .call()
    });
    match handle.join() {
        Ok(Ok(response)) => {
            let status = response.status();
            let mut body = String::new();
            response
                .into_reader()
                .take(MAX_FETCH_BYTES)
                .read_to_string(&mut body)
                .map_err(ToolError::Io)?;
            Ok((status, body))
        }
        Ok(Err(e)) => Err(ToolError::InvalidInput(format!(
            "web_fetch: request failed: {e}"
        ))),
        Err(_) => Err(ToolError::InvalidInput(
            "web_fetch: request panicked or timed out".into(),
        )),
    }
}

/// Returns (title, plain_text) extracted from HTML.
/// Strips tags, skips <script>/<style> bodies, decodes five common entities.
fn strip_html(html: &str) -> (String, String) {
    let mut title = String::new();
    let mut text = String::new();
    let mut in_tag = false;
    let mut skip_depth: usize = 0;
    let mut in_title = false;
    let mut tag_buf = String::new();
    let mut last_was_space = true;

    let chars: Vec<char> = html.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];

        if ch == '<' {
            in_tag = true;
            tag_buf.clear();
            i += 1;
            continue;
        }

        if in_tag {
            if ch == '>' {
                in_tag = false;
                let tag = tag_buf.trim().to_ascii_lowercase();
                let tag_name = tag
                    .trim_start_matches('/')
                    .split_whitespace()
                    .next()
                    .unwrap_or("");

                let is_closing = tag.starts_with('/');

                if matches!(tag_name, "script" | "style") {
                    if is_closing {
                        skip_depth = skip_depth.saturating_sub(1);
                    } else {
                        skip_depth += 1;
                    }
                } else if tag_name == "title" {
                    in_title = !is_closing;
                } else if matches!(
                    tag_name,
                    "p" | "div"
                        | "br"
                        | "h1"
                        | "h2"
                        | "h3"
                        | "h4"
                        | "h5"
                        | "h6"
                        | "li"
                        | "tr"
                        | "td"
                        | "th"
                ) && !in_title
                    && skip_depth == 0
                    && !last_was_space
                {
                    text.push(' ');
                    last_was_space = true;
                }
                tag_buf.clear();
            } else {
                tag_buf.push(ch);
            }
            i += 1;
            continue;
        }

        // Outside tags — check for entity sequences
        if ch == '&' {
            // Try to consume an entity
            let rest: String = chars[i..].iter().take(8).collect();
            let (entity_char, consumed) = decode_entity(&rest);
            if let Some(ec) = entity_char {
                if skip_depth == 0 {
                    if in_title {
                        title.push(ec);
                    } else if ec == ' ' || ec == '\n' {
                        if !last_was_space {
                            text.push(' ');
                            last_was_space = true;
                        }
                    } else {
                        text.push(ec);
                        last_was_space = false;
                    }
                }
                i += consumed;
                continue;
            }
        }

        if skip_depth > 0 {
            i += 1;
            continue;
        }

        if in_title {
            title.push(ch);
            i += 1;
            continue;
        }

        if ch == '\n' || ch == '\r' || ch == '\t' || ch == ' ' {
            if !last_was_space {
                text.push(' ');
                last_was_space = true;
            }
        } else {
            text.push(ch);
            last_was_space = false;
        }

        i += 1;
    }

    (title.trim().to_string(), text.trim().to_string())
}

/// Returns (Some(char), chars_consumed) if a known entity is recognised, else (None, 0).
fn decode_entity(s: &str) -> (Option<char>, usize) {
    if s.starts_with("&amp;") {
        return (Some('&'), 5);
    }
    if s.starts_with("&lt;") {
        return (Some('<'), 4);
    }
    if s.starts_with("&gt;") {
        return (Some('>'), 4);
    }
    if s.starts_with("&quot;") {
        return (Some('"'), 6);
    }
    if s.starts_with("&#39;") {
        return (Some('\''), 5);
    }
    if s.starts_with("&nbsp;") {
        return (Some(' '), 6);
    }
    (None, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_is_immediate() {
        let tool = WebFetchTool::new(true);
        let spec = tool.spec();
        assert_eq!(spec.name, "web_fetch");
        assert_eq!(spec.execution_kind, ExecutionKind::Immediate);
        assert!(spec.default_risk.is_none());
    }

    #[test]
    fn invalid_scheme_rejected() {
        let err = validate_url("ftp://example.com").unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(ref m) if m.contains("http://")));
    }

    #[test]
    fn no_scheme_rejected() {
        let err = validate_url("example.com").unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(ref m) if m.contains("http://")));
    }

    #[test]
    fn private_ip_blocked() {
        let err = validate_url("http://127.0.0.1/").unwrap_err();
        assert!(
            matches!(err, ToolError::InvalidInput(ref m) if m.contains("not permitted")),
            "expected not permitted error, got: {err:?}"
        );
    }

    #[test]
    fn loopback_ip_v4_all_forms() {
        assert!(is_private_ip("127.0.0.1".parse().unwrap()));
        assert!(is_private_ip("127.255.255.255".parse().unwrap()));
    }

    #[test]
    fn private_ranges_blocked() {
        assert!(is_private_ip("10.0.0.1".parse().unwrap()));
        assert!(is_private_ip("172.16.0.1".parse().unwrap()));
        assert!(is_private_ip("172.31.255.255".parse().unwrap()));
        assert!(is_private_ip("192.168.1.1".parse().unwrap()));
        assert!(is_private_ip("169.254.1.1".parse().unwrap()));
        assert!(!is_private_ip("172.15.0.1".parse().unwrap()));
        assert!(!is_private_ip("172.32.0.1".parse().unwrap()));
    }

    #[test]
    fn loopback_v6_blocked() {
        assert!(is_private_ip("::1".parse().unwrap()));
    }

    #[test]
    fn public_ip_allowed() {
        assert!(!is_private_ip("1.1.1.1".parse().unwrap()));
        assert!(!is_private_ip("8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn strip_html_basic() {
        let (title, text) = strip_html("<html><body><p>Hello world</p></body></html>");
        assert!(title.is_empty());
        assert!(text.contains("Hello world"));
    }

    #[test]
    fn strip_html_extracts_title() {
        let (title, _) =
            strip_html("<html><head><title>My Page</title></head><body>text</body></html>");
        assert_eq!(title, "My Page");
    }

    #[test]
    fn strip_html_skips_script() {
        let (_, text) = strip_html("<body><script>var x = secret();</script><p>visible</p></body>");
        assert!(
            !text.contains("secret"),
            "script body must not appear in output"
        );
        assert!(text.contains("visible"));
    }

    #[test]
    fn strip_html_skips_style() {
        let (_, text) = strip_html("<style>.hidden { display:none }</style><p>shown</p>");
        assert!(
            !text.contains("hidden"),
            "style body must not appear in output"
        );
        assert!(text.contains("shown"));
    }

    #[test]
    fn strip_html_decodes_entities() {
        let (_, text) = strip_html("<p>&amp; &lt; &gt; &quot; &#39; &nbsp;</p>");
        assert!(text.contains('&'));
        assert!(text.contains('<'));
        assert!(text.contains('>'));
        assert!(text.contains('"'));
        assert!(text.contains('\''));
    }

    #[test]
    fn extract_host_basic() {
        assert_eq!(
            extract_host("https://example.com/path"),
            Some("example.com".into())
        );
        assert_eq!(
            extract_host("http://example.com:8080/"),
            Some("example.com".into())
        );
        assert_eq!(
            extract_host("http://example.com"),
            Some("example.com".into())
        );
    }

    #[test]
    fn extract_host_no_scheme_returns_none() {
        assert_eq!(extract_host("example.com"), None);
    }

    #[test]
    fn disabled_tool_returns_error() {
        use crate::runtime::{ProjectPath, ProjectRoot, ResolvedToolInput};
        use std::path::PathBuf;
        let tool = WebFetchTool::new(false);
        let root = ProjectRoot::new(PathBuf::from(".")).unwrap();
        let path = ProjectPath::from_trusted(root.path().to_path_buf(), ".".to_string());
        let _ = path; // suppress unused warning
        let input = ResolvedToolInput::WebFetch {
            url: "https://example.com".into(),
        };
        let err = tool.run(&input).unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(ref m) if m.contains("disabled")));
    }
}
