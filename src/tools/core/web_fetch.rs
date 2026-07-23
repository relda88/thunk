use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs};

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

fn is_private_ip(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(a) => is_private_ipv4(a),
        IpAddr::V6(a) => {
            // ::ffff:a.b.c.d addresses must be checked as their embedded
            // IPv4 address, not against the IPv6-only rules below.
            if let Some(mapped) = a.to_ipv4_mapped() {
                return is_private_ipv4(mapped);
            }
            a.is_loopback()
                || a.is_multicast()
                || a.is_unicast_link_local() // fe80::/10 (stable stdlib; correct /10 boundary, unlike a hand-rolled segment check)
                || a.is_unique_local() // fc00::/7 (ULA)
        }
    }
}

fn is_private_ipv4(a: Ipv4Addr) -> bool {
    let o = a.octets();
    a.is_private() // RFC 1918: 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16
        || a.is_loopback() // 127.0.0.0/8
        || a.is_link_local() // 169.254.0.0/16
        || a.is_broadcast() // 255.255.255.255
        || a.is_multicast() // 224.0.0.0/4
        || o[0] == 0 // 0.0.0.0/8, "this network" — not just the all-zero address
        || (o[0] == 100 && (64..=127).contains(&o[1])) // 100.64.0.0/10 CGNAT; Ipv4Addr::is_shared() is unstable, so this is a manual range check
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
    // Only a literal IP in the URL can be checked here without performing a
    // DNS lookup. A hostname is deliberately NOT resolved in this function:
    // resolving it here and again inside fetch_url would be two independent
    // resolutions of the same name, reopening the DNS-rebinding TOCTOU
    // window this guard exists to close. Hostnames are validated exactly
    // once, at actual connect time, by SsrfGuardResolver in fetch_url —
    // that is the single point of truth for hostname-based validation,
    // covering both the initial connection and every redirect hop.
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_private_ip(ip) {
            return Err(ToolError::InvalidInput(
                "web_fetch: private/loopback addresses not permitted".into(),
            ));
        }
    }
    Ok(())
}

/// Rejects the entire resolution if any candidate address is private/blocked.
/// Must not silently filter down to just the public addresses: if a
/// hostname resolves to a mix of public and private addresses, filtering
/// would let an attacker win a race by controlling which address ureq
/// happens to connect to first.
fn validate_resolved_addrs(addrs: &[SocketAddr], netloc: &str) -> std::io::Result<()> {
    if addrs.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("web_fetch: no addresses resolved for {netloc}"),
        ));
    }
    if addrs.iter().any(|a| is_private_ip(a.ip())) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("web_fetch: private/loopback address blocked for {netloc}"),
        ));
    }
    Ok(())
}

/// Custom DNS resolver installed on the ureq Agent used by fetch_url.
///
/// ureq's connect_host() (src/stream.rs in the ureq crate) is the sole
/// TCP-connect function for both the initial request AND every redirect hop
/// — each redirect rebuilds a Unit for the new location and re-enters
/// connect_host, which calls this resolver again for the new host before
/// opening a socket. Installing the guard here, rather than as a one-time
/// pre-check, makes it the single enforcement point for every address
/// actually connected to, including redirect targets, without needing to
/// disable redirects (`.redirects(0)`) or re-implement following them
/// manually.
struct SsrfGuardResolver;

impl ureq::Resolver for SsrfGuardResolver {
    fn resolve(&self, netloc: &str) -> std::io::Result<Vec<SocketAddr>> {
        let addrs: Vec<SocketAddr> = netloc.to_socket_addrs()?.collect();
        validate_resolved_addrs(&addrs, netloc)?;
        Ok(addrs)
    }
}

fn fetch_url(url: &str) -> Result<(u16, String), ToolError> {
    let url = url.to_string();
    let handle = std::thread::spawn(move || {
        let agent = ureq::AgentBuilder::new()
            .resolver(SsrfGuardResolver)
            .build();
        agent
            .get(&url)
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
    fn ipv4_mapped_ipv6_checked_as_embedded_v4() {
        assert!(is_private_ip("::ffff:127.0.0.1".parse().unwrap()));
        assert!(is_private_ip("::ffff:10.0.0.1".parse().unwrap()));
        assert!(is_private_ip("::ffff:169.254.1.1".parse().unwrap()));
        assert!(!is_private_ip("::ffff:8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn ipv6_ula_blocked() {
        assert!(is_private_ip("fc00::1".parse().unwrap()));
        assert!(is_private_ip(
            "fdff:ffff:ffff:ffff:ffff:ffff:ffff:ffff".parse().unwrap()
        ));
        assert!(!is_private_ip("fe00::1".parse().unwrap()));
    }

    #[test]
    fn cgnat_range_blocked() {
        assert!(is_private_ip("100.64.0.0".parse().unwrap()));
        assert!(is_private_ip("100.64.0.1".parse().unwrap()));
        assert!(is_private_ip("100.127.255.255".parse().unwrap()));
        assert!(!is_private_ip("100.63.255.255".parse().unwrap()));
        assert!(!is_private_ip("100.128.0.0".parse().unwrap()));
    }

    #[test]
    fn broadcast_blocked() {
        assert!(is_private_ip("255.255.255.255".parse().unwrap()));
    }

    #[test]
    fn multicast_blocked() {
        assert!(is_private_ip("224.0.0.1".parse().unwrap()));
        assert!(is_private_ip("239.255.255.255".parse().unwrap()));
        assert!(is_private_ip("ff02::1".parse().unwrap()));
        assert!(is_private_ip("ff0e::1".parse().unwrap()));
    }

    #[test]
    fn full_this_network_range_blocked() {
        assert!(is_private_ip("0.0.0.0".parse().unwrap()));
        assert!(is_private_ip("0.1.2.3".parse().unwrap()));
        assert!(is_private_ip("0.255.255.255".parse().unwrap()));
    }

    #[test]
    fn ipv6_link_local_full_slash10_range_blocked() {
        // Previously the check was `segments()[0] == 0xfe80` exact equality,
        // which missed most of fe80::/10 (0xFE80..=0xFEBF). These two cases
        // are exactly what that bug missed.
        assert!(is_private_ip("fe90::1".parse().unwrap()));
        assert!(is_private_ip(
            "febf:ffff:ffff:ffff:ffff:ffff:ffff:ffff".parse().unwrap()
        ));
        assert!(is_private_ip("fe80::1".parse().unwrap()));
        assert!(!is_private_ip("fec0::1".parse().unwrap()));
    }

    #[test]
    fn multi_homed_resolution_rejects_if_any_private() {
        let public: SocketAddr = "1.1.1.1:80".parse().unwrap();
        let private: SocketAddr = "10.0.0.5:80".parse().unwrap();
        let err = validate_resolved_addrs(&[public, private], "mixed.test:80").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn multi_homed_resolution_rejects_regardless_of_order() {
        let public: SocketAddr = "8.8.8.8:80".parse().unwrap();
        let private: SocketAddr = "192.168.1.1:80".parse().unwrap();
        // private-first ordering must be rejected identically to private-last —
        // the check must not just look at addrs[0].
        let err = validate_resolved_addrs(&[private, public], "mixed.test:80").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn multi_homed_resolution_allows_all_public() {
        let a: SocketAddr = "1.1.1.1:80".parse().unwrap();
        let b: SocketAddr = "8.8.8.8:80".parse().unwrap();
        assert!(validate_resolved_addrs(&[a, b], "public.test:80").is_ok());
    }

    /// Test-only resolver: identical to `SsrfGuardResolver` except it treats
    /// one exact `SocketAddr` (the loopback-bound local test server started
    /// below) as pre-trusted, since a real "public" host isn't available in
    /// a sandboxed test run. Every other address — including the redirect
    /// target — goes through the real `validate_resolved_addrs` guard.
    struct TestExceptResolver {
        allow_exact: SocketAddr,
    }

    impl ureq::Resolver for TestExceptResolver {
        fn resolve(&self, netloc: &str) -> std::io::Result<Vec<SocketAddr>> {
            let addrs: Vec<SocketAddr> = netloc.to_socket_addrs()?.collect();
            if addrs == [self.allow_exact] {
                return Ok(addrs);
            }
            validate_resolved_addrs(&addrs, netloc)?;
            Ok(addrs)
        }
    }

    #[test]
    fn redirect_to_private_ip_is_blocked() {
        use std::io::Write;
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = std::io::Read::read(&mut stream, &mut buf);
                // 169.254.169.254 is the canonical cloud-metadata SSRF
                // target; it's already blocked as link-local even before
                // this slice's other fixes, so this isolates redirect-hop
                // enforcement specifically.
                let response =
                    "HTTP/1.1 302 Found\r\nLocation: http://169.254.169.254/\r\nContent-Length: 0\r\n\r\n";
                let _ = stream.write_all(response.as_bytes());
            }
        });

        let agent = ureq::AgentBuilder::new()
            .resolver(TestExceptResolver { allow_exact: addr })
            .build();

        let err = agent
            .get(&format!("http://127.0.0.1:{}/", addr.port()))
            .timeout(std::time::Duration::from_secs(5))
            .call()
            .expect_err("redirect to a private/link-local address must be rejected");

        let msg = err.to_string();
        assert!(
            msg.contains("private/loopback address blocked"),
            "expected the redirect target to be rejected by the resolver guard, got: {msg}"
        );
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
