// HTTPS requests through curl, configured on stdin so URLs, credentials, and bodies never appear
// in a process list or a log: synchronous requests verify integrations, detached ones deliver
// alerts without delaying a hook.

use std::io::Write;
use std::process::{Child, Command, Stdio};

/** Quote a value for curl's configuration syntax
 * Input
    - value: &str - text
 * Output
    - String quoted for a curl config file
*/
fn quoted(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
    )
}

/** Build the curl configuration of a request
 * Input
    - method: &str - GET or POST
    - url: &str - endpoint
    - headers: &[String] - headers, credentials included
    - body: Option<&str> - request body
    - max_time: u64 - time limit in seconds
 * Output
    - String curl configuration
*/
fn config(
    method: &str,
    url: &str,
    headers: &[String],
    body: Option<&str>,
    max_time: u64,
) -> String {
    let mut config = format!(
        "url = {}\nrequest = {}\nsilent\nshow-error\nmax-time = {max_time}\n",
        quoted(url),
        quoted(method)
    );
    for header in headers {
        config.push_str(&format!("header = {}\n", quoted(header)));
    }
    if let Some(body) = body {
        config.push_str(&format!("data = {}\n", quoted(body)));
    }
    config
}

/** Start curl with a configuration on stdin
 * Input
    - config: &str - curl configuration
    - capture: bool - capture stdout and stderr (otherwise they are discarded)
 * Output
    - Result<Child, String>
    - Error if curl is missing
*/
fn spawn(config: &str, capture: bool) -> Result<Child, String> {
    let output = || {
        if capture {
            Stdio::piped()
        } else {
            Stdio::null()
        }
    };
    let mut child = super::process::hidden(&mut Command::new("curl"))
        .args(["-K", "-"])
        .stdin(Stdio::piped())
        .stdout(output())
        .stderr(output())
        .spawn()
        .map_err(|error| format!("curl is unavailable: {error}"))?;
    child
        .stdin
        .take()
        .ok_or("curl has no stdin")?
        .write_all(config.as_bytes())
        .map_err(|error| error.to_string())?;
    Ok(child)
}

/** Send a request and wait for the answer (at most 20 seconds)
 * Input
    - method: &str - GET or POST
    - url: &str - endpoint
    - headers: &[String] - headers, credentials included
    - body: Option<&str> - request body
 * Output
    - Result<(u16, String), String> status code and response body
    - Error if curl is missing or the request could not be sent
*/
pub(crate) fn request(
    method: &str,
    url: &str,
    headers: &[String],
    body: Option<&str>,
) -> Result<(u16, String), String> {
    request_within(method, url, headers, body, 20)
}

/** Send a request and wait for the answer, at most a given time
 * Input
    - method: &str - GET or POST
    - url: &str - endpoint
    - headers: &[String] - headers, credentials included
    - body: Option<&str> - request body
    - seconds: u64 - time limit
 * Output
    - Result<(u16, String), String> status code and response body
    - Error if curl is missing or the request could not be sent
*/
pub(crate) fn request_within(
    method: &str,
    url: &str,
    headers: &[String],
    body: Option<&str>,
    seconds: u64,
) -> Result<(u16, String), String> {
    let mut config = config(method, url, headers, body, seconds);
    config.push_str("write-out = \"\\n%{http_code}\"\n");
    let output = spawn(&config, true)?
        .wait_with_output()
        .map_err(|error| error.to_string())?;
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    let (body, status) = text.rsplit_once('\n').unwrap_or(("", text.as_str()));
    match status.trim().parse::<u16>() {
        Ok(status) if status > 0 => Ok((status, body.to_string())),
        _ => Err(format!(
            "the request did not complete: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )),
    }
}

/** Send a JSON POST without waiting for the answer (at most 10 seconds in the background)
 * Input
    - url: &str - endpoint
    - headers: &[String] - extra headers, credentials included
    - body: &str - JSON body
 * Output
    - Result<(), String>
    - Error if curl is missing
*/
pub(crate) fn post_detached(url: &str, headers: &[String], body: &str) -> Result<(), String> {
    let mut all = vec!["Content-Type: application/json".to_string()];
    all.extend(headers.iter().cloned());
    spawn(&config("POST", url, &all, Some(body), 10), false).map(|_| ())
}

/** Encode bytes as standard base64 (for HTTP basic authentication)
 * Input
    - bytes: &[u8] - data
 * Output
    - String
*/
pub(crate) fn base64(bytes: &[u8]) -> String {
    /** The standard base64 alphabet */
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::new();
    for chunk in bytes.chunks(3) {
        let value = chunk.iter().enumerate().fold(0u32, |value, (index, byte)| {
            value | (u32::from(*byte) << (16 - 8 * index))
        });
        for index in 0..4 {
            if index <= chunk.len() {
                output.push(ALPHABET[(value >> (18 - 6 * index) & 63) as usize] as char);
            } else {
                output.push('=');
            }
        }
    }
    output
}

// Only the ingestion service decodes request bodies
#[cfg(any(test, feature = "server"))]
/** Decode standard base64 (padding required, no whitespace)
 * Input
    - text: &[u8] - encoded data
 * Output
    - Result<Vec<u8>, String> the decoded bytes
    - Error if the text is not valid base64
*/
pub(crate) fn unbase64(text: &[u8]) -> Result<Vec<u8>, String> {
    let invalid = || "invalid base64".to_string();
    if !text.len().is_multiple_of(4) {
        return Err(invalid());
    }
    let value = |byte: u8| match byte {
        b'A'..=b'Z' => Some(u32::from(byte - b'A')),
        b'a'..=b'z' => Some(u32::from(byte - b'a') + 26),
        b'0'..=b'9' => Some(u32::from(byte - b'0') + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let mut output = Vec::with_capacity(text.len() / 4 * 3);
    let chunks = text.len() / 4;
    for (number, chunk) in text.chunks(4).enumerate() {
        // Padding may only end the last chunk: "xx==" or "xxx="
        let padding = chunk.iter().rev().take_while(|byte| **byte == b'=').count();
        if padding > 2 || (padding > 0 && number + 1 != chunks) {
            return Err(invalid());
        }
        let mut bits = 0u32;
        for (index, byte) in chunk[..4 - padding].iter().enumerate() {
            bits |= value(*byte).ok_or_else(invalid)? << (18 - 6 * index);
        }
        output.extend_from_slice(&bits.to_be_bytes()[1..4 - padding]);
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::{base64, config, unbase64};

    /** Check base64 against known vectors
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn encodes_base64() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"user@x.com:token"), "dXNlckB4LmNvbTp0b2tlbg==");
    }

    /** Check that decoding inverts encoding and rejects malformed text
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn decodes_base64() {
        for text in [
            &b""[..],
            b"f",
            b"fo",
            b"foo",
            b"@('a') | python -m x; if ($?) { y }",
        ] {
            assert_eq!(unbase64(base64(text).as_bytes()).unwrap(), text);
        }
        let all = (0..=255u8).collect::<Vec<_>>();
        assert_eq!(unbase64(base64(&all).as_bytes()).unwrap(), all);
        for bad in ["Zg=", "Z===", "Zg==Zm8=", "Zm9v!A==", "Zm 9v"] {
            assert!(unbase64(bad.as_bytes()).is_err(), "{bad}");
        }
    }

    /** Check that configuration values are quoted so they cannot inject curl options
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn quotes_configuration() {
        let text = config(
            "POST",
            "https://x/\"\nurl = evil",
            &[],
            Some("{\"a\":1}"),
            5,
        );
        assert!(
            text.contains("url = \"https://x/\\\"\\nurl = evil\""),
            "{text}"
        );
        assert_eq!(
            text.matches("\nurl = ").count() + usize::from(text.starts_with("url = ")),
            1
        );
        assert!(text.contains("data = \"{\\\"a\\\":1}\""));
    }
}
