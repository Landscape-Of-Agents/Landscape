// Model token usage, read from the agent's own transcript (Claude Code passes its path to every
// hook). Only the numbers of each model turn's usage are read: input, output, cache read, and
// cache creation tokens, and the model's name. No prompt, reply, or tool content is kept. A tool
// call is attributed an equal share of the turn that issued it (a turn that issues three tool
// calls gives each a third), so the shares of a session add up to the tokens of its tool-issuing
// turns. Transcripts in another format (Codex) give nothing, and the measurements say so.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde_json::Value;

/** How much of a transcript's end is read to find one tool call */
const TAIL_BYTES: u64 = 4 * 1024 * 1024;

/** Largest transcript read whole for the session totals */
const MAX_BYTES: u64 = 64 * 1024 * 1024;

/** Token usage of one model turn, or of a share of it, or of a whole session
 * Fields
    - input: f64 - input tokens (not read from the cache)
    - output: f64 - output tokens
    - cache_read: f64 - input tokens read from the prompt cache
    - cache_creation: f64 - input tokens written to the prompt cache
    - calls: u64 - tool calls the turn issued (for a session: every tool call)
    - turns: u64 - model turns counted
    - model: Option<String> - the model of the turn (the last one, for a session)
*/
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Usage {
    pub(crate) input: f64,
    pub(crate) output: f64,
    pub(crate) cache_read: f64,
    pub(crate) cache_creation: f64,
    pub(crate) calls: u64,
    pub(crate) turns: u64,
    pub(crate) model: Option<String>,
}

impl Usage {
    /** Every token the usage counts
     * Input
        - None (uses self)
     * Output
        - f64
    */
    pub(crate) fn total(&self) -> f64 {
        self.input + self.output + self.cache_read + self.cache_creation
    }

    /** The equal share of one tool call of the turn
     * Input
        - None (uses self)
     * Output
        - Usage
    */
    pub(crate) fn share(&self) -> Usage {
        let calls = self.calls.max(1) as f64;
        Usage {
            input: self.input / calls,
            output: self.output / calls,
            cache_read: self.cache_read / calls,
            cache_creation: self.cache_creation / calls,
            ..self.clone()
        }
    }
}

/** Read the end of a file (or all of it, up to a size)
 * Input
    - path: &Path - file
    - limit: u64 - most bytes read, from the end
 * Output
    - Option<String>, None when the file cannot be read
*/
fn tail(path: &Path, limit: u64) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let start = length.saturating_sub(limit);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.take(limit).read_to_end(&mut bytes).ok()?;
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    // Drop a line cut by the start of the read
    if start > 0 {
        text = text.split_once('\n').map(|(_, rest)| rest.to_string())?;
    }
    Some(text)
}

/** One assistant line of a transcript: its message id, usage, model, and tool calls
 * Input
    - line: &Value - transcript line
 * Output
    - Option<(String, Usage, Vec<String>)>, None for lines that are not assistant messages with usage
*/
fn assistant(line: &Value) -> Option<(String, Usage, Vec<String>)> {
    if line["type"] != "assistant" {
        return None;
    }
    let message = &line["message"];
    let id = message["id"].as_str()?.to_string();
    let usage = &message["usage"];
    usage.as_object()?;
    let number = |key: &str| usage[key].as_f64().unwrap_or(0.0);
    let calls = message["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|block| block["type"] == "tool_use")
        .filter_map(|block| block["id"].as_str().map(String::from))
        .collect();
    Some((
        id,
        Usage {
            input: number("input_tokens"),
            output: number("output_tokens"),
            cache_read: number("cache_read_input_tokens"),
            cache_creation: number("cache_creation_input_tokens"),
            calls: 0,
            turns: 1,
            model: message["model"].as_str().map(String::from),
        },
        calls,
    ))
}

/** Merge the lines of each model turn (a turn is written as one line per content block, each
 * repeating the usage, whose output grows while it streams): the largest counts, every tool call
 * Input
    - text: &str - transcript lines
 * Output
    - BTreeMap<String, (Usage, Vec<String>)> per message id
*/
fn turns(text: &str) -> BTreeMap<String, (Usage, Vec<String>)> {
    let mut found: BTreeMap<String, (Usage, Vec<String>)> = BTreeMap::new();
    for line in text.lines() {
        if !line.contains("\"assistant\"") {
            continue;
        }
        let Some((id, usage, calls)) = serde_json::from_str::<Value>(line)
            .ok()
            .as_ref()
            .and_then(assistant)
        else {
            continue;
        };
        let entry = found
            .entry(id)
            .or_insert_with(|| (usage.clone(), Vec::new()));
        entry.0.input = entry.0.input.max(usage.input);
        entry.0.output = entry.0.output.max(usage.output);
        entry.0.cache_read = entry.0.cache_read.max(usage.cache_read);
        entry.0.cache_creation = entry.0.cache_creation.max(usage.cache_creation);
        if usage.model.is_some() {
            entry.0.model = usage.model;
        }
        for call in calls {
            if !entry.1.contains(&call) {
                entry.1.push(call);
            }
        }
    }
    for (usage, calls) in found.values_mut() {
        usage.calls = calls.len() as u64;
    }
    found
}

/** The usage of the model turn that issued a tool call
 * Input
    - transcript: &Path - the agent's transcript (JSON lines)
    - tool_use_id: &str - the tool call
 * Output
    - Option<Usage> of the whole turn (calls says how many tool calls share it), None when the
      transcript cannot be read or does not hold the call
*/
pub(crate) fn of_tool_call(transcript: &Path, tool_use_id: &str) -> Option<Usage> {
    let text = tail(transcript, TAIL_BYTES)?;
    turns(&text)
        .into_values()
        .find(|(_, calls)| calls.iter().any(|call| call == tool_use_id))
        .map(|(usage, _)| usage)
}

/** The usage of a whole session: every model turn of its transcript added up
 * Input
    - transcript: &Path - the agent's transcript
 * Output
    - Option<Usage>, None when it cannot be read or holds no usage
*/
pub(crate) fn of_session(transcript: &Path) -> Option<Usage> {
    let text = tail(transcript, MAX_BYTES)?;
    let found = turns(&text);
    if found.is_empty() {
        return None;
    }
    let mut total = Usage::default();
    for (usage, _) in found.values() {
        total.input += usage.input;
        total.output += usage.output;
        total.cache_read += usage.cache_read;
        total.cache_creation += usage.cache_creation;
        total.calls += usage.calls;
        total.turns += 1;
        if usage.model.is_some() {
            total.model = usage.model.clone();
        }
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    /** A turn split over lines, a turn issuing two calls, prose lines, and broken lines
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn usage_is_attributed_to_tool_calls() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("transcript.jsonl");
        let lines = [
            r#"{"type":"user","message":{"content":"secret prompt text"}}"#,
            r#"{"type":"assistant","message":{"id":"m1","model":"claude-opus-5-5","content":[{"type":"text","text":"thinking"}],"usage":{"input_tokens":100,"output_tokens":1,"cache_read_input_tokens":900}}}"#,
            r#"{"type":"assistant","message":{"id":"m1","model":"claude-opus-5-5","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{}}],"usage":{"input_tokens":100,"output_tokens":40,"cache_read_input_tokens":900,"cache_creation_input_tokens":60}}}"#,
            "not json",
            r#"{"type":"assistant","message":{"id":"m2","model":"claude-opus-5-5","content":[{"type":"tool_use","id":"t2"},{"type":"tool_use","id":"t3"}],"usage":{"input_tokens":30,"output_tokens":80}}}"#,
            r#"{"type":"assistant","message":{"id":"m3","content":[{"type":"text","text":"done"}],"usage":{"input_tokens":5,"output_tokens":7}}}"#,
        ];
        std::fs::write(&path, lines.join("\n")).unwrap();
        let first = of_tool_call(&path, "t1").unwrap();
        assert_eq!(
            (
                first.input,
                first.output,
                first.cache_read,
                first.cache_creation,
                first.calls
            ),
            (100.0, 40.0, 900.0, 60.0, 1),
            "the streamed turn's largest counts"
        );
        assert_eq!(first.model.as_deref(), Some("claude-opus-5-5"));
        let second = of_tool_call(&path, "t3").unwrap();
        assert_eq!(second.calls, 2);
        assert_eq!(second.share().output, 40.0, "two calls share the turn");
        assert!(of_tool_call(&path, "t9").is_none());
        let session = of_session(&path).unwrap();
        assert_eq!((session.turns, session.calls), (3, 3));
        assert_eq!(session.output, 127.0);
        assert_eq!(session.total(), 135.0 + 127.0 + 900.0 + 60.0);
        assert!(of_session(&directory.path().join("missing.jsonl")).is_none());
    }
}
