// The Foxx dashboard in local mode: 'crane dashboard' serves a read-only page and JSON API on
// 127.0.0.1 only. The browser is paired with a random per-run token passed in the URL fragment
// (never sent to the server in the page request, never logged); every API request must present it.
// Requests are GET only; the Host header must name the loopback address and port (DNS rebinding),
// a present Origin must be the dashboard itself (cross-site requests), and responses carry a strict
// Content-Security-Policy. The API can only read: it has no route that runs commands, changes
// policies, approves, edits budgets, or reads arbitrary files.

pub(crate) mod api;
pub(crate) mod views;

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::time::Duration;

use serde_json::{json, Value};

use crate::governance::workspace::Workspace;
use crate::platform::files::append_line;
use crate::platform::now_millis;
use crate::trust::crypto::{hex, random_bytes};

/** The dashboard page */
const PAGE: &str = include_str!("app.html");

/** Most headers accepted in one request */
const MAX_HEADERS: usize = 64;

/** Longest request line or header accepted, in bytes */
const MAX_LINE: usize = 8192;

/** Compare two secrets in constant time
 * Input
    - left: &str - secret
    - right: &str - secret
 * Output
    - bool
*/
fn same(left: &str, right: &str) -> bool {
    left.len() == right.len()
        && left
            .bytes()
            .zip(right.bytes())
            .fold(0u8, |difference, (a, b)| difference | (a ^ b))
            == 0
}

/** Open a URL in the default browser
 * Input
    - url: &str - URL
 * Output
    - Result<(), String>
*/
fn open_browser(url: &str) -> Result<(), String> {
    let status = if cfg!(windows) {
        Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", url])
            .status()
    } else if cfg!(target_os = "macos") {
        Command::new("open").arg(url).status()
    } else {
        Command::new("xdg-open").arg(url).status()
    };
    match status {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!("the browser launcher exited with {status}")),
        Err(error) => Err(error.to_string()),
    }
}

/** Serve the dashboard until stopped: bind to the loopback address (CRANE_DASHBOARD_PORT or a
 * free port), print the paired URL, open it in the browser (unless CRANE_DASHBOARD_NO_BROWSER=1),
 * and answer requests (at most CRANE_DASHBOARD_MAX_REQUESTS when set)
 * Input
    - None
 * Output
    - Result<(), String>
    - Error if Crane is not initialized or the address cannot be bound
*/
pub(crate) fn serve() -> Result<(), String> {
    let workspace = Workspace::locate()?;
    let token = hex(&random_bytes(24)?);
    let port = std::env::var("CRANE_DASHBOARD_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(0);
    let listener = TcpListener::bind(("127.0.0.1", port)).map_err(|error| {
        format!("the dashboard could not open: cannot listen on 127.0.0.1:{port}: {error}")
    })?;
    let port = listener
        .local_addr()
        .map_err(|error| error.to_string())?
        .port();
    let url = format!("http://127.0.0.1:{port}/#token={token}");
    println!(
        "Foxx dashboard (read-only) for {}: {url}",
        workspace.root.display()
    );
    println!("Press Ctrl+C to stop.");
    std::io::stdout().flush().ok();
    if std::env::var("CRANE_DASHBOARD_NO_BROWSER").map_or(true, |value| value != "1") {
        if let Err(error) = open_browser(&url) {
            eprintln!("crane: could not open a browser ({error}); open the URL above manually");
        }
    }
    if crate::telemetry::sync::configured(&workspace) {
        let stores = crate::telemetry::Stores::open(&workspace.trust()?.runtime());
        match crate::telemetry::ingest::sync(&workspace, &stores) {
            Ok(flushed) => println!(
                "Foxx service: synchronized {} events, {} ledger entries, {} governance changes, {} alerts; the dashboard reads from the service",
                flushed.events, flushed.ledger, flushed.audit, flushed.alerts
            ),
            Err(error) => eprintln!(
                "crane: synchronization failed ({error}); the dashboard serves the local evidence, which remains authoritative"
            ),
        }
        let interval = crate::telemetry::sync::interval_seconds();
        let background = workspace.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(interval));
            let _ = crate::telemetry::ingest::sync(&background, &stores);
        });
    }
    let limit = std::env::var("CRANE_DASHBOARD_MAX_REQUESTS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok());
    let mut served = 0;
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        handle(stream, &workspace, &token, port);
        served += 1;
        if limit.is_some_and(|limit| served >= limit) {
            break;
        }
    }
    Ok(())
}

/** Answer one connection and log the access (path and status only)
 * Input
    - stream: TcpStream - connection
    - workspace: &Workspace - repository
    - token: &str - pairing token
    - port: u16 - bound port
 * Output
    - None
*/
fn handle(mut stream: TcpStream, workspace: &Workspace, token: &str, port: u16) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let (status, content_type, body, path, download) =
        match read(&mut stream, workspace, token, port) {
            Ok((status, content_type, body, path, download)) => {
                (status, content_type, body, path, download)
            }
            Err((status, message)) => (
                status,
                "application/json",
                json!({"error": message}).to_string(),
                String::new(),
                None,
            ),
        };
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        421 => "Misdirected Request",
        _ => "Internal Server Error",
    };
    let disposition = download
        .map(|name| format!("Content-Disposition: attachment; filename=\"{name}\"\r\n"))
        .unwrap_or_default();
    let _ = write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'; img-src data:; frame-ancestors 'none'; base-uri 'none'; form-action 'none'\r\n{disposition}Connection: close\r\n\r\n{body}",
        body.len()
    );
    if let Ok(trust) = workspace.trust() {
        let _ = append_line(
            &trust.runtime().join("dashboard-access.jsonl"),
            &json!({"at": now_millis(), "path": path.split('?').next().unwrap_or_default(), "status": status}).to_string(),
        );
    }
}

/** Parse a query string
 * Input
    - query: &str - text after '?'
 * Output
    - BTreeMap<String, String>
*/
fn parse_query(query: &str) -> BTreeMap<String, String> {
    query
        .split('&')
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (!key.is_empty()).then(|| (decode(key), decode(value)))
        })
        .collect()
}

/** Decode percent-encoding and '+' in a query component
 * Input
    - text: &str - encoded text
 * Output
    - String
*/
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => output.push(b' '),
            b'%' if index + 2 < bytes.len() => {
                let digits = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("zz");
                match u8::from_str_radix(digits, 16) {
                    Ok(byte) => {
                        output.push(byte);
                        index += 2;
                    }
                    Err(_) => output.push(b'%'),
                }
            }
            byte => output.push(byte),
        }
        index += 1;
    }
    String::from_utf8_lossy(&output).into_owned()
}

/** The response produced for a request: status, content type, body, path, download name */
type Response = (u16, &'static str, String, String, Option<String>);

/** Read one request and produce its response
 * Input
    - stream: &mut TcpStream - connection
    - workspace: &Workspace - repository
    - token: &str - pairing token
    - port: u16 - bound port
 * Output
    - Result<Response, (u16, String)>
*/
fn read(
    stream: &mut TcpStream,
    workspace: &Workspace,
    token: &str,
    port: u16,
) -> Result<Response, (u16, String)> {
    let mut reader = BufReader::new(
        stream
            .try_clone()
            .map_err(|error| (400, error.to_string()))?,
    );
    let mut line = String::new();
    reader
        .by_ref()
        .take(MAX_LINE as u64)
        .read_line(&mut line)
        .map_err(|error| (400, error.to_string()))?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default().to_string();
    let mut headers = BTreeMap::new();
    loop {
        let mut header = String::new();
        reader
            .by_ref()
            .take(MAX_LINE as u64)
            .read_line(&mut header)
            .map_err(|error| (400, error.to_string()))?;
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
        if headers.len() > MAX_HEADERS {
            return Err((400, "too many headers".into()));
        }
    }
    if method != "GET" {
        return Err((
            405,
            "the Foxx dashboard is read-only; governance changes are made with the Crane CLI"
                .into(),
        ));
    }
    let allowed_hosts = [format!("127.0.0.1:{port}"), format!("localhost:{port}")];
    if !headers
        .get("host")
        .is_some_and(|host| allowed_hosts.contains(&host.to_ascii_lowercase()))
    {
        return Err((
            421,
            "the request does not address the local dashboard (Host header rejected)".into(),
        ));
    }
    if let Some(origin) = headers.get("origin") {
        if !allowed_hosts
            .iter()
            .any(|host| origin.eq_ignore_ascii_case(&format!("http://{host}")))
        {
            return Err((403, "cross-origin requests are refused".into()));
        }
    }
    if headers
        .get("sec-fetch-site")
        .is_some_and(|site| site == "cross-site")
    {
        return Err((403, "cross-site requests are refused".into()));
    }
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    if path == "/" || path == "/index.html" {
        return Ok((200, "text/html", PAGE.to_string(), path.to_string(), None));
    }
    if !path.starts_with("/api/") {
        return Err((404, "not found".into()));
    }
    let presented = headers
        .get("authorization")
        .and_then(|value| value.strip_prefix("Bearer "))
        .or_else(|| headers.get("x-foxx-token").map(String::as_str))
        .unwrap_or_default();
    if !same(presented, token) {
        return Err((
            401,
            "missing or wrong dashboard token; open the URL printed by 'crane dashboard'".into(),
        ));
    }
    let query = parse_query(query);
    let (status, body, download) = api::route(workspace, path, &query);
    let body = match body {
        Value::String(text) if download.is_some() => text,
        other => other.to_string(),
    };
    Ok((status, "application/json", body, path.to_string(), download))
}

#[cfg(test)]
mod tests {
    use super::*;

    /** Check query parsing and percent-decoding
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn parses_queries() {
        let query = parse_query("session=ses_1&type=tool_call.decided&q=a%20b+c&bad=%zz");
        assert_eq!(query["session"], "ses_1");
        assert_eq!(query["q"], "a b c");
        assert_eq!(query["bad"], "%zz");
        assert!(same("abc", "abc") && !same("abc", "abd") && !same("abc", "ab"));
    }
}
