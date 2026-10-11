// The autonomy-credit ledger: risk-bearing authority, kept apart from tokens, cost, and compute.
// It is append-only and hash-chained; every entry records the deltas, the balances before and
// after, the reason, the registry generation, the issuer, and an idempotency key, so a retried
// hook can never charge twice and every balance can be reconstructed from the entries alone.
// A balance never grants a permission or overrides a denial: an exhausted balance only turns an
// otherwise allowed action into REQUIRE_APPROVAL.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::platform::files::{append_line, read_lines, Lock};
use crate::platform::{io_error, now_millis};
use crate::trust::crypto::digest_json;

/** Ledger operation
 * Variants
    - Grant - credits issued to a session
    - Reserve - credits held for an authorized action
    - Consume - a reservation settled after the action ran
    - Release - a reservation returned (the action did not run)
    - Refund - consumed credits returned
    - Regenerate - credits restored by a controlled event
    - Expire - unused credits removed at the end of their lifetime
    - Revoke - credits removed after a violation
    - Override - a human set the balance
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum Kind {
    Grant,
    Reserve,
    Consume,
    Release,
    Refund,
    Regenerate,
    Expire,
    Revoke,
    Override,
}

/** A session's credit balances
 * Fields
    - granted: u64 - total granted and regenerated
    - available: u64 - spendable now
    - reserved: u64 - held by authorized actions not yet settled
    - consumed: u64 - spent
    - released: u64 - returned from reservations
    - refunded: u64 - returned after consumption
    - expired: u64 - expired
    - revoked: u64 - revoked
*/
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Balance {
    pub(crate) granted: u64,
    pub(crate) available: u64,
    pub(crate) reserved: u64,
    pub(crate) consumed: u64,
    pub(crate) released: u64,
    pub(crate) refunded: u64,
    pub(crate) expired: u64,
    pub(crate) revoked: u64,
}

impl Balance {
    /** Apply an operation, clamping so a balance never goes negative
     * Input
        - kind: Kind - operation
        - amount: u64 - requested amount
     * Output
        - (Balance, u64) the new balance and the amount actually applied
    */
    pub(crate) fn apply(&self, kind: Kind, amount: u64) -> (Self, u64) {
        let mut next = *self;
        let applied = match kind {
            Kind::Grant | Kind::Regenerate => {
                next.granted += amount;
                next.available += amount;
                amount
            }
            Kind::Reserve => {
                let applied = amount.min(next.available);
                next.available -= applied;
                next.reserved += applied;
                applied
            }
            Kind::Consume => {
                let applied = amount.min(next.reserved);
                next.reserved -= applied;
                next.consumed += applied;
                applied
            }
            Kind::Release => {
                let applied = amount.min(next.reserved);
                next.reserved -= applied;
                next.available += applied;
                next.released += applied;
                applied
            }
            Kind::Refund => {
                let applied = amount.min(next.consumed);
                next.consumed -= applied;
                next.available += applied;
                next.refunded += applied;
                applied
            }
            Kind::Expire => {
                let applied = amount.min(next.available);
                next.available -= applied;
                next.expired += applied;
                applied
            }
            Kind::Revoke => {
                let applied = amount.min(next.available);
                next.available -= applied;
                next.revoked += applied;
                applied
            }
            Kind::Override => {
                next.available = amount;
                amount
            }
        };
        (next, applied)
    }
}

/** One ledger entry
 * Fields
    - entry_id: String - identifier
    - seq: u64 - position in the ledger
    - at: u64 - Unix milliseconds
    - session: String - canonical session
    - task: Option<String> - canonical task
    - tool_call_id: Option<String> - tool call
    - kind: Kind - operation
    - requested: u64 - amount requested
    - amount: u64 - amount applied
    - before: Balance - balances before
    - after: Balance - balances after
    - reason: String - why
    - registry_generation: u64 - policy version in force
    - issuer: String - crane, or the human for overrides
    - idempotency_key: String - unique per operation
    - previous: String - digest of the previous entry
    - digest: String - SHA-512 of this entry without the digest field
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Entry {
    pub(crate) entry_id: String,
    pub(crate) seq: u64,
    pub(crate) at: u64,
    pub(crate) session: String,
    pub(crate) task: Option<String>,
    pub(crate) tool_call_id: Option<String>,
    pub(crate) kind: Kind,
    pub(crate) requested: u64,
    pub(crate) amount: u64,
    pub(crate) before: Balance,
    pub(crate) after: Balance,
    pub(crate) reason: String,
    pub(crate) registry_generation: u64,
    pub(crate) issuer: String,
    pub(crate) idempotency_key: String,
    pub(crate) previous: String,
    pub(crate) digest: String,
}

/** Compute an entry digest
 * Input
    - entry: &Entry - entry
 * Output
    - String of 128 hex characters
*/
fn entry_digest(entry: &Entry) -> String {
    let mut value = serde_json::to_value(entry).unwrap_or(json!(null));
    if let Some(object) = value.as_object_mut() {
        object.remove("digest");
    }
    digest_json("telemetry/ledger/v1", &value)
}

/** A requested ledger operation
 * Fields
    - session: &'a str - canonical session
    - task: Option<&'a str> - canonical task
    - tool_call_id: Option<&'a str> - tool call
    - kind: Kind - operation
    - amount: u64 - amount
    - reason: &'a str - why
    - registry_generation: u64 - policy version
    - idempotency_key: &'a str - unique key
*/
#[derive(Debug, Clone, Copy)]
pub(crate) struct Operation<'a> {
    pub(crate) session: &'a str,
    pub(crate) task: Option<&'a str>,
    pub(crate) tool_call_id: Option<&'a str>,
    pub(crate) kind: Kind,
    pub(crate) amount: u64,
    pub(crate) reason: &'a str,
    pub(crate) registry_generation: u64,
    pub(crate) idempotency_key: &'a str,
}

/** The ledger of one repository
 * Fields
    - path: PathBuf - ledger.jsonl in the runtime directory
*/
#[derive(Debug, Clone)]
pub(crate) struct Ledger {
    path: PathBuf,
}

impl Ledger {
    /** Open the ledger in a runtime directory
     * Input
        - directory: &Path - runtime directory
     * Output
        - Ledger
    */
    pub(crate) fn open(directory: &Path) -> Self {
        Self {
            path: directory.join("ledger.jsonl"),
        }
    }

    /** Read every entry
     * Input
        - None (uses self)
     * Output
        - Result<Vec<Entry>, String>
    */
    pub(crate) fn entries(&self) -> Result<Vec<Entry>, String> {
        read_lines(&self.path)?
            .into_iter()
            .map(|value| serde_json::from_value(value).map_err(io_error))
            .collect()
    }

    /** Reconstruct a session's balance from the entries alone
     * Input
        - entries: &[Entry] - ledger entries
        - session: &str - canonical session
     * Output
        - Balance
    */
    pub(crate) fn balance_of(entries: &[Entry], session: &str) -> Balance {
        entries
            .iter()
            .filter(|entry| entry.session == session)
            .fold(Balance::default(), |balance, entry| {
                balance.apply(entry.kind, entry.requested).0
            })
    }

    /** Apply an operation atomically and idempotently: under the ledger lock, an operation whose
     * idempotency key exists returns the existing entry; otherwise the balance is reconstructed,
     * the operation applied, and the entry appended
     * Input
        - operation: Operation - what to apply
     * Output
        - Result<(Entry, bool), String> the entry and whether it was newly written
    */
    pub(crate) fn apply(&self, operation: Operation) -> Result<(Entry, bool), String> {
        let _lock = Lock::acquire(&self.path.with_extension("lock"))?;
        let entries = self.entries()?;
        if let Some(existing) = entries
            .iter()
            .find(|entry| entry.idempotency_key == operation.idempotency_key)
        {
            return Ok((existing.clone(), false));
        }
        let before = Self::balance_of(&entries, operation.session);
        let (after, amount) = before.apply(operation.kind, operation.amount);
        let mut entry = Entry {
            entry_id: String::new(),
            seq: entries.len() as u64 + 1,
            at: now_millis(),
            session: operation.session.into(),
            task: operation.task.map(String::from),
            tool_call_id: operation.tool_call_id.map(String::from),
            kind: operation.kind,
            requested: operation.amount,
            amount,
            before,
            after,
            reason: operation.reason.into(),
            registry_generation: operation.registry_generation,
            issuer: "crane".into(),
            idempotency_key: operation.idempotency_key.into(),
            previous: entries
                .last()
                .map_or_else(|| "genesis".to_string(), |entry| entry.digest.clone()),
            digest: String::new(),
        };
        entry.entry_id = format!(
            "led_{}",
            &digest_json("telemetry/ledger-id/v1", &json!(operation.idempotency_key))[..24]
        );
        entry.digest = entry_digest(&entry);
        append_line(
            &self.path,
            &serde_json::to_string(&entry).map_err(io_error)?,
        )?;
        Ok((entry, true))
    }
}

/** Verify a ledger: the chain is intact and every recorded balance matches the reconstruction
 * Input
    - entries: &[Entry] - entries in order
 * Output
    - Result<(), String>
    - Error naming the first entry that fails
*/
#[cfg_attr(not(feature = "server"), allow(dead_code))] // used by the hosted dashboard
pub(crate) fn verify(entries: &[Entry]) -> Result<(), String> {
    let mut previous = "genesis".to_string();
    let mut keys = std::collections::BTreeSet::new();
    for (index, entry) in entries.iter().enumerate() {
        if entry.seq != index as u64 + 1
            || entry.previous != previous
            || entry_digest(entry) != entry.digest
        {
            return Err(format!(
                "ledger entry {} was modified, removed, or reordered",
                index + 1
            ));
        }
        if !keys.insert(entry.idempotency_key.as_str()) {
            return Err(format!(
                "ledger entry {} repeats an idempotency key (double charge)",
                index + 1
            ));
        }
        let before = Ledger::balance_of(&entries[..index], &entry.session);
        if before != entry.before || before.apply(entry.kind, entry.requested).0 != entry.after {
            return Err(format!(
                "ledger entry {} records balances that do not reconcile",
                index + 1
            ));
        }
        previous = entry.digest.clone();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /** Build an operation for tests
     * Input
        - kind: Kind - operation
        - amount: u64 - amount
        - key: &str - idempotency key
     * Output
        - Operation
    */
    fn operation(kind: Kind, amount: u64, key: &str) -> Operation<'_> {
        Operation {
            session: "ses_a",
            task: None,
            tool_call_id: None,
            kind,
            amount,
            reason: "test",
            registry_generation: 1,
            idempotency_key: key,
        }
    }

    /** Check reservation, settlement, idempotency, clamping, and reconciliation
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn ledger_is_idempotent_and_reconcilable() {
        let directory = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(directory.path());
        ledger.apply(operation(Kind::Grant, 10, "g")).unwrap();
        ledger.apply(operation(Kind::Reserve, 4, "r1")).unwrap();
        let (_, fresh) = ledger.apply(operation(Kind::Reserve, 4, "r1")).unwrap();
        assert!(!fresh, "a retried reservation must not charge twice");
        ledger.apply(operation(Kind::Consume, 4, "c1")).unwrap();
        ledger.apply(operation(Kind::Reserve, 100, "r2")).unwrap();
        let entries = ledger.entries().unwrap();
        let balance = Ledger::balance_of(&entries, "ses_a");
        assert_eq!(
            (balance.available, balance.reserved, balance.consumed),
            (0, 6, 4)
        );
        assert!(verify(&entries).is_ok());
        let mut tampered = entries;
        tampered[1].after.available = 100;
        assert!(verify(&tampered).is_err());
    }
}
