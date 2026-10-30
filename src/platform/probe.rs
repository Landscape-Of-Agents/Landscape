// Checking that an address points somewhere before Crane keeps it: an integration's URL, or a URL in
// a subagent's parameters. The syntax must be an https URL (http only for this machine itself), and
// the address must answer: any HTTP answer that shows the service is there counts (2xx, 3xx, and the
// 401, 403, 405 an API gives without credentials); no answer, a 404 or 410, or a 5xx does not.
// The machine checks with curl, from its own network, so an internal service is fine there. The
// hosted dashboard checks with its own client and refuses private, loopback, and link-local
// addresses (server/dashboard/probe.rs), so it cannot be used to reach anything inside its network.

use std::net::IpAddr;

use serde_json::Value;

/** Longest a check waits for an answer (seconds) */
pub(crate) const PROBE_SECONDS: u64 = 10;

/** Check an address's syntax: https, a host, no spaces (http only for localhost)
 * Input
    - url: &str - address
 * Output
    - Result<(), String>
*/
pub(crate) fn syntax(url: &str) -> Result<(), String> {
    let url = url.trim();
    let rest = if let Some(rest) = url.strip_prefix("https://") {
        rest
    } else if let Some(rest) = url.strip_prefix("http://") {
        let host = rest.split(['/', ':', '?', '#']).next().unwrap_or_default();
        if host != "localhost" && host != "127.0.0.1" {
            return Err(format!(
                "'{url}' must be an https URL (http only for localhost)"
            ));
        }
        rest
    } else {
        return Err(format!("'{url}' must start with https://"));
    };
    if rest.is_empty()
        || rest.starts_with('/')
        || url
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(format!("'{url}' is not a full URL"));
    }
    Ok(())
}

/** The host and port of an address
 * Input
    - url: &str - address (syntax already checked)
 * Output
    - Option<(String, u16)>
*/
pub(crate) fn host_and_port(url: &str) -> Option<(String, u16)> {
    let (scheme, rest) = url.trim().split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let default = if scheme == "https" { 443 } else { 80 };
    if let Some(inner) = authority.strip_prefix('[') {
        let (host, after) = inner.split_once(']')?;
        let port = after
            .strip_prefix(':')
            .map_or(Some(default), |port| port.parse().ok())?;
        return Some((host.to_string(), port));
    }
    match authority.rsplit_once(':') {
        Some((host, port)) => Some((host.to_string(), port.parse().ok()?)),
        None => Some((authority.to_string(), default)),
    }
}

/** What an HTTP status says about an address
 * Input
    - url: &str - address
    - status: u16 - HTTP status
 * Output
    - Result<String, String> - why it counts as pointing correctly, or why not
*/
pub(crate) fn verdict(url: &str, status: u16) -> Result<String, String> {
    match status {
        200..=399 => Ok(format!("{url} answered HTTP {status}")),
        401 | 403 | 405 => Ok(format!(
            "{url} answered HTTP {status} (the service is there; it needs the credentials)"
        )),
        404 | 410 => Err(format!(
            "{url} answered HTTP {status}: nothing is at that path; check the address"
        )),
        500..=599 => Err(format!(
            "{url} answered HTTP {status}: the service has an error; try again or check the address"
        )),
        other => Err(format!("{url} answered HTTP {other}; check the address")),
    }
}

/** Check from this machine that an address answers (syntax, then a request)
 * Input
    - url: &str - address
 * Output
    - Result<String, String> - what answered, or why it does not point anywhere
*/
pub(crate) fn check(url: &str) -> Result<String, String> {
    syntax(url)?;
    let (status, _) = super::http::request_within(
        "GET",
        url.trim(),
        &["User-Agent: crane-url-check".to_string()],
        None,
        PROBE_SECONDS,
    )
    .map_err(|error| format!("{} did not answer: {error}", url.trim()))?;
    verdict(url.trim(), status)
}

/** Report whether an address is on the public internet (not private, loopback, link-local,
 * multicast, unspecified, documentation, or carrier-grade NAT)
 * Input
    - ip: IpAddr - address
 * Output
    - bool
*/
#[cfg_attr(not(feature = "server"), allow(dead_code))] // used by the hosted dashboard's check
pub(crate) fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, _, _] = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_multicast()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || a == 0
                || (a == 100 && (64..128).contains(&b))
                || (a == 198 && (b == 18 || b == 19)))
        }
        IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_public(IpAddr::V4(mapped));
            }
            let first = v6.segments()[0];
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80
                || first == 0x2001 && v6.segments()[1] == 0x0db8)
        }
    }
}

/** Every string in a JSON value that is an address (starts with http:// or https://)
 * Input
    - value: &Value - settings or parameters
 * Output
    - Vec<String>
*/
pub(crate) fn urls_in(value: &Value) -> Vec<String> {
    let mut found = Vec::new();
    let mut walk = vec![value];
    while let Some(item) = walk.pop() {
        match item {
            Value::String(text) if text.starts_with("http://") || text.starts_with("https://") => {
                found.push(text.trim().to_string())
            }
            Value::Array(items) => walk.extend(items.iter()),
            Value::Object(map) => walk.extend(map.values()),
            _ => {}
        }
    }
    found.sort();
    found.dedup();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    /** Addresses are checked for syntax, their host and port read, statuses judged, private
     * addresses told apart, and URLs found in nested values
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn addresses_are_judged_before_they_are_kept() {
        assert!(syntax("https://grafana.example.com/d/abc").is_ok());
        assert!(syntax("http://localhost:3000").is_ok());
        assert!(syntax("http://grafana.example.com").is_err());
        assert!(syntax("ftp://x").is_err());
        assert!(syntax("https:// spaced").is_err());
        assert_eq!(
            host_and_port("https://user@api.example.com:8443/x"),
            Some(("api.example.com".into(), 8443))
        );
        assert_eq!(host_and_port("https://[::1]/x"), Some(("::1".into(), 443)));
        assert!(verdict("u", 200).is_ok() && verdict("u", 401).is_ok());
        assert!(verdict("u", 404).is_err() && verdict("u", 502).is_err());
        assert!(!is_public("10.0.0.5".parse().unwrap()));
        assert!(!is_public("169.254.169.254".parse().unwrap()));
        assert!(!is_public("::1".parse().unwrap()));
        assert!(!is_public("::ffff:192.168.1.1".parse().unwrap()));
        assert!(is_public("140.82.112.3".parse().unwrap()));
        let urls = urls_in(
            &serde_json::json!({"a": "https://x.example", "b": ["http://localhost:1", 3], "c": "plain"}),
        );
        assert_eq!(urls, ["http://localhost:1", "https://x.example"]);
    }
}
