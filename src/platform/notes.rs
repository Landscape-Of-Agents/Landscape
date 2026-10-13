// Notes: facts found deep inside Crane, where no event store is at hand (a signing key unlocked or
// sealed, an owner key looked up or trusted, a policy rule's test result), that belong to the event
// the running hook or command records next. Each note is a small JSON object with a "kind"; the
// event store attaches the pending notes to the next event it stores (payload "notes"), and the
// event specification (spec.rs) turns them into their own DASHBOARD.md events.

use std::sync::Mutex;

use serde_json::{json, Value};

/** Notes waiting for the next stored event of this process */
static PENDING: Mutex<Vec<Value>> = Mutex::new(Vec::new());

/** Most notes kept for one event (a policy test of a large repository stays bounded) */
const MAX_NOTES: usize = 200;

/** Record a note for the next stored event
 * Input
    - kind: &str - note kind, such as KEY_UNLOCKED
    - fields: Value - its facts (an object; never secrets or source content)
 * Output
    - None
*/
pub(crate) fn note(kind: &str, fields: Value) {
    let mut note = json!({"kind": kind});
    if let (Some(target), Some(source)) = (note.as_object_mut(), fields.as_object()) {
        for (key, value) in source {
            target.insert(key.clone(), value.clone());
        }
    }
    if let Ok(mut pending) = PENDING.lock() {
        if pending.len() < MAX_NOTES {
            pending.push(note);
        }
    }
}

/** Take every pending note
 * Input
    - None
 * Output
    - Vec<Value>
*/
pub(crate) fn drain() -> Vec<Value> {
    PENDING
        .lock()
        .map(|mut pending| std::mem::take(&mut *pending))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /** Check that notes are kept in order and taken once
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn notes_are_taken_once() {
        drain();
        note("KEY_UNLOCKED", json!({"protection": "DPAPI"}));
        note("OWNER_KEY_LOOKUP", json!({"result": "MATCH"}));
        let taken = drain();
        assert_eq!(taken.len(), 2);
        assert_eq!(taken[0]["kind"], "KEY_UNLOCKED");
        assert_eq!(taken[0]["protection"], "DPAPI");
        assert!(drain().is_empty());
    }
}
