// HTTP plumbing shared by the upload API and the dashboard: one Reply type for every response
// (JSON, the page, its script and images, downloads, redirects), request headers and cookies,
// query decoding, the request's correlation identifier, and sending a reply with the headers every
// response carries (Cache-Control, X-Request-Id).

use std::collections::BTreeMap;

use serde_json::Value;
use tiny_http::{Header, Request, Response};

use crate::trust::crypto::{hex, random_bytes};

/** A response
 * Fields
    - status: u16 - HTTP status
    - content_type: &'static str - media type
    - body: Vec<u8> - body
    - cacheable: bool - may be cached for a day (static assets); otherwise no-store
    - headers: Vec<(String, String)> - extra headers
*/
pub(crate) struct Reply {
    pub(crate) status: u16,
    content_type: &'static str,
    body: Vec<u8>,
    cacheable: bool,
    headers: Vec<(String, String)>,
}

impl Reply {
    /** Build a reply
     * Input
        - status: u16 - HTTP status
        - content_type: &'static str - media type
        - body: Vec<u8> - body
     * Output
        - Reply
    */
    fn new(status: u16, content_type: &'static str, body: Vec<u8>) -> Self {
        Self {
            status,
            content_type,
            body,
            cacheable: false,
            headers: Vec::new(),
        }
    }

    /** A JSON response
     * Input
        - status: u16 - HTTP status
        - value: Value - body
     * Output
        - Reply
    */
    pub(crate) fn json(status: u16, value: Value) -> Self {
        Self::new(status, "application/json", value.to_string().into_bytes())
    }

    /** A JSON or CSV file download
     * Input
        - text: String - file content
        - name: &str - file name offered to the browser
     * Output
        - Reply
    */
    pub(crate) fn download(text: String, name: &str) -> Self {
        let content_type = if name.ends_with(".csv") {
            "text/csv; charset=utf-8"
        } else {
            "application/json"
        };
        Self::new(200, content_type, text.into_bytes()).with_header(
            "Content-Disposition",
            &format!("attachment; filename=\"{name}\""),
        )
    }

    /** The dashboard page, with a strict content security policy: only its own script and images,
     * same-origin requests, no frames, no forms
     * Input
        - body: &str - page
     * Output
        - Reply
    */
    pub(crate) fn page(body: &str) -> Self {
        Self::new(200, "text/html; charset=utf-8", body.as_bytes().to_vec())
            .with_header("Content-Security-Policy", "default-src 'none'; script-src 'self'; connect-src 'self'; style-src 'unsafe-inline'; img-src 'self'; form-action 'none'; frame-ancestors 'none'; base-uri 'none'")
            .with_header("X-Content-Type-Options", "nosniff")
            .with_header("Referrer-Policy", "no-referrer")
    }

    /** The dashboard script
     * Input
        - body: &str - script
     * Output
        - Reply
    */
    pub(crate) fn script(body: &str) -> Self {
        Self::new(
            200,
            "text/javascript; charset=utf-8",
            body.as_bytes().to_vec(),
        )
        .with_header("X-Content-Type-Options", "nosniff")
    }

    /** A static image, cacheable for a day
     * Input
        - bytes: &[u8] - image
        - content_type: &'static str - media type
     * Output
        - Reply
    */
    pub(crate) fn image(bytes: &[u8], content_type: &'static str) -> Self {
        let mut reply = Self::new(200, content_type, bytes.to_vec())
            .with_header("X-Content-Type-Options", "nosniff");
        reply.cacheable = true;
        reply
    }

    /** A redirect
     * Input
        - location: &str - target
     * Output
        - Reply
    */
    pub(crate) fn redirect(location: &str) -> Self {
        Self::new(302, "text/plain", Vec::new()).with_header("Location", location)
    }

    /** Add a header
     * Input
        - name: &str - header name
        - value: &str - header value
     * Output
        - Reply
    */
    pub(crate) fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

/** Send a reply with the headers every response carries
 * Input
    - request: Request - the request being answered
    - reply: Reply - the answer
    - request_id: &str - the request's correlation identifier
 * Output
    - None (a client that went away is not an error worth reporting)
*/
pub(crate) fn send(request: Request, reply: Reply, request_id: &str) {
    let mut response = Response::from_data(reply.body).with_status_code(reply.status);
    let cache = if reply.cacheable {
        "public, max-age=86400"
    } else {
        "no-store"
    };
    let common = [
        ("Content-Type".to_string(), reply.content_type.to_string()),
        ("Cache-Control".to_string(), cache.to_string()),
        ("X-Request-Id".to_string(), request_id.to_string()),
    ];
    for (field, value) in common.into_iter().chain(reply.headers) {
        if let Ok(header) = Header::from_bytes(field.as_bytes(), value.as_bytes()) {
            response.add_header(header);
        }
    }
    let _ = request.respond(response);
}

/** Read a request header
 * Input
    - request: &Request - request
    - name: &'static str - header name
 * Output
    - Option<String> trimmed value
*/
pub(crate) fn header(request: &Request, name: &'static str) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|header| header.field.equiv(name))
        .map(|header| header.value.as_str().trim().to_string())
}

/** Read a cookie
 * Input
    - request: &Request - request
    - name: &str - cookie name
 * Output
    - Option<String>
*/
pub(crate) fn cookie(request: &Request, name: &str) -> Option<String> {
    request
        .headers()
        .iter()
        .filter(|header| header.field.equiv("Cookie"))
        .flat_map(|header| header.value.as_str().split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value.to_string())
}

/** Return the request's correlation identifier: the caller's X-Request-Id when it is 8 to 64
 * letters, digits, or dashes (Crane sends one with every upload), otherwise a new random one
 * Input
    - request: &Request - request
 * Output
    - String
*/
pub(crate) fn request_id(request: &Request) -> String {
    header(request, "X-Request-Id")
        .filter(|id| {
            (8..=64).contains(&id.len())
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
        .unwrap_or_else(|| random_bytes(8).map(|bytes| hex(&bytes)).unwrap_or_default())
}

/** Return the client's address: the first X-Forwarded-For entry (set by the hosting platform's
 * proxy), else the connection's address
 * Input
    - request: &Request - request
 * Output
    - String
*/
pub(crate) fn client_address(request: &Request) -> String {
    header(request, "X-Forwarded-For")
        .and_then(|list| list.split(',').next().map(|first| first.trim().to_string()))
        .or_else(|| {
            request
                .remote_addr()
                .map(|address| address.ip().to_string())
        })
        .unwrap_or_default()
}

/** Decode %XX escapes and + in a query value
 * Input
    - text: &str - encoded text
 * Output
    - String
*/
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' => match text
                .get(index + 1..index + 3)
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
            {
                Some(byte) => {
                    out.push(byte);
                    index += 3;
                    continue;
                }
                None => out.push(b'%'),
            },
            b'+' => out.push(b' '),
            byte => out.push(byte),
        }
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/** Split a URL into its path and decoded query parameters
 * Input
    - url: &str - request URL
 * Output
    - (String, BTreeMap<String, String>)
*/
pub(crate) fn split(url: &str) -> (String, BTreeMap<String, String>) {
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    let parameters = query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(key, value)| (decode(key), decode(value)))
        .collect();
    (path.to_string(), parameters)
}

#[cfg(test)]
mod tests {
    use super::{decode, split};

    /** Check query decoding and splitting
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn queries_are_decoded() {
        assert_eq!(decode("a%2Fb+c%zz"), "a/b c%zz");
        let (path, query) = split("/api/events?type=tool_call&members=ab%2Ccd");
        assert_eq!(path, "/api/events");
        assert_eq!(query["members"], "ab,cd");
        assert_eq!(query["type"], "tool_call");
    }
}
