// Structured logs of the service: one JSON object per line on stderr, which hosting platforms
// collect and index. Each line has a timestamp, a level, the component, the request's correlation
// identifier (also returned to the client as X-Request-Id, and sent by Crane with every upload),
// a message, and fields. Log lines never contain secrets: database errors pass through redaction
// first, request lines record the path without its query, and signatures and cookies are never
// logged.

use std::io::Write;

use serde_json::{json, Value};

use crate::platform::now_millis;
use crate::platform::render::iso_time;

/** Write one log line
 * Input
    - level: &str - info, warn, or error
    - component: &str - http, ingest, dashboard, or service
    - request_id: Option<&str> - correlation identifier
    - message: &str - what happened (no secrets)
    - fields: Value - extra structured fields (an object)
 * Output
    - None (a log line that cannot be written is dropped)
*/
pub(crate) fn write(
    level: &str,
    component: &str,
    request_id: Option<&str>,
    message: &str,
    fields: Value,
) {
    let mut line = json!({
        "ts": iso_time(now_millis()),
        "level": level,
        "component": component,
        "message": message,
    });
    if let Some(id) = request_id {
        line["request_id"] = json!(id);
    }
    if let Value::Object(fields) = fields {
        for (key, value) in fields {
            line[key] = value;
        }
    }
    let _ = writeln!(std::io::stderr().lock(), "{line}");
}
