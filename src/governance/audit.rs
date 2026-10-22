// .crane/audit.jsonl: the append-only, hash-chained audit trail of governance changes (selections created and moved, checkpoints, defaults, context bindings). The chain head is covered by the signed manifest, so an edited, removed, or reordered entry is detected.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::trust::crypto::digest_json;

/** Chain value before the first entry */
pub(crate) const GENESIS: &str = "genesis";

/** Digest domain of an audit entry */
const AUDIT_DOMAIN: &str = "governance/audit/v1";

/** One audit entry
 * Fields
    - seq: u64 - 1-based position
    - at: u64 - Unix seconds
    - actor: String - who made the change
    - action: String - what changed, such as selection.created
    - detail: Value - structured facts (identifiers, names, digests; never source content)
    - previous: String - digest of the previous entry, GENESIS for the first
    - digest: String - SHA-512 of this entry without the digest field
*/
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Entry {
    pub(crate) seq: u64,
    pub(crate) at: u64,
    pub(crate) actor: String,
    pub(crate) action: String,
    pub(crate) detail: Value,
    pub(crate) previous: String,
    pub(crate) digest: String,
}

/** Compute an entry's digest from its other fields
 * Input
    - entry: &Entry - audit entry
 * Output
    - String of 128 hex characters
*/
fn entry_digest(entry: &Entry) -> String {
    digest_json(
        AUDIT_DOMAIN,
        &json!({
            "seq": entry.seq,
            "at": entry.at,
            "actor": entry.actor,
            "action": entry.action,
            "detail": entry.detail,
            "previous": entry.previous,
        }),
    )
}

/** Return the head of an audit chain
 * Input
    - entries: &[Entry] - entries in order
 * Output
    - String, GENESIS when empty
*/
pub(crate) fn head(entries: &[Entry]) -> String {
    entries
        .last()
        .map_or_else(|| GENESIS.to_string(), |entry| entry.digest.clone())
}

/** Build the next entry of a chain
 * Input
    - entries: &[Entry] - existing entries
    - at: u64 - Unix seconds
    - actor: &str - who made the change
    - action: &str - what changed
    - detail: Value - structured facts
 * Output
    - Entry chained to the current head
*/
pub(crate) fn next(entries: &[Entry], at: u64, actor: &str, action: &str, detail: Value) -> Entry {
    let mut entry = Entry {
        seq: entries.len() as u64 + 1,
        at,
        actor: actor.into(),
        action: action.into(),
        detail,
        previous: head(entries),
        digest: String::new(),
    };
    entry.digest = entry_digest(&entry);
    entry
}

/** Verify an audit chain: sequence numbers run 1, 2, 3, and every digest is correct and chains
 * from the entry before
 * Input
    - entries: &[Entry] - entries in order
 * Output
    - Result<(), String>
    - Error naming the first broken entry
*/
pub(crate) fn verify(entries: &[Entry]) -> Result<(), String> {
    let mut previous = GENESIS.to_string();
    for (index, entry) in entries.iter().enumerate() {
        if entry.seq != index as u64 + 1
            || entry.previous != previous
            || entry_digest(entry) != entry.digest
        {
            return Err(format!(
                "audit entry {} was edited, removed, or reordered outside Crane",
                index + 1
            ));
        }
        previous = entry.digest.clone();
    }
    Ok(())
}
