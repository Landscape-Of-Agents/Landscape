// The Selection Registry: one authoritative, signed record per selection in .crane/registry.json,
// and the human-readable .crane/map from each 8-character identifier to its 512-bit origin digest.
//
// Every record carries three separate SHA-512 digests, each under its own domain: the content
// digest of the exact selected bytes (line endings normalized), the origin-binding digest of the
// repository identity, checkpoint, blob, original path and range, and content digest, and the
// record digest of the record's canonical JSON. The record digest is signed with Ed25519; the
// origin fields never change after creation, and every accepted change (relocation, policy move)
// produces a new record generation and a new registry generation. The identifier is a lookup key
// only: authority comes from a record whose signature verifies against a trusted key.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::trust::crypto::{
    digest, digest_json, is_digest, is_selection_id, verify_signature, SigningIdentity,
};

/** Version of the registry file layout */
pub(crate) const REGISTRY_FORMAT: u64 = 1;

/** Digest domain of selected content */
pub(crate) const CONTENT_DOMAIN: &str = "selection/content/v1";

/** Digest domain of the origin binding */
pub(crate) const ORIGIN_DOMAIN: &str = "selection/origin/v1";

/** Digest and signature domain of a selection record */
pub(crate) const RECORD_DOMAIN: &str = "selection/record/v1";

/** What a policy command does with a selection; the marker itself carries no operation
 * Variants
    - Preserve - the selected content must stay identical to its trusted baseline
    - Target - the selected content must change, meaningfully and within the selection
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Operation {
    Preserve,
    Target,
}

impl Operation {
    /** Return the keyword used in policies and messages
     * Input
        - None (uses self)
     * Output
        - &'static str, "preserve" or "target"
    */
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Preserve => "preserve",
            Self::Target => "target",
        }
    }
}

/** Where selected content lies in one version of a file
 * Fields
    - path: String - repository-relative path with "/" separators
    - start_line: usize - first selected line (1-based, markers excluded)
    - end_line: usize - last selected line
    - start_byte: usize - byte offset of the selected content
    - end_byte: usize - byte offset just past the selected content
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Span {
    pub(crate) path: String,
    pub(crate) start_line: usize,
    pub(crate) end_line: usize,
    pub(crate) start_byte: usize,
    pub(crate) end_byte: usize,
}

/** The immutable creation facts of a selection, resolved against a trusted checkpoint
 * Fields
    - checkpoint: String - checkpoint name
    - commit: String - checkpoint commit SHA
    - blob: String - Git blob id of the file in that commit
    - span: Span - the selected range in the checkpoint version of the file
    - content_digest: String - SHA-512 (content domain) of the selected baseline content
    - binding_digest: String - SHA-512 (origin domain) binding repository, checkpoint, blob,
      span, and content digest
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Origin {
    pub(crate) checkpoint: String,
    pub(crate) commit: String,
    pub(crate) blob: String,
    pub(crate) span: Span,
    pub(crate) content_digest: String,
    pub(crate) binding_digest: String,
}

/** The latest accepted resolution of a selection in the work tree
 * Fields
    - span: Span - where the markers enclose the selection now
    - content_digest: String - SHA-512 (content domain) of the content at that resolution
    - resolved_at_generation: u64 - registry generation that recorded this resolution
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Current {
    pub(crate) span: Span,
    pub(crate) content_digest: String,
    pub(crate) resolved_at_generation: u64,
}

/** Lifecycle of a selection record
 * Variants
    - Active - the selection is bound and enforced
    - Unresolved - its identity could not be re-established; every policy using it fails closed
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub(crate) enum Status {
    Active,
    Unresolved,
}

/** One signed selection record
 * Fields
    - id: String - 8-character identifier written in the markers
    - name: Option<String> - optional human-readable name
    - context: Option<String> - guidance for agents working in the selection (required since 0.4.23;
      absent in older records, which keep their signed digests)
    - repository_id: String - identity digest of the repository the selection belongs to
    - operation: Operation - preserve or target, as bound by the policy command
    - change_type: Option<String> - required change kind of a target
    - policy: String - name of the policy holding the command
    - language: String - comment adapter used for the markers
    - origin: Origin - immutable creation facts
    - current: Current - latest accepted resolution
    - status: Status - active or unresolved
    - generation: u64 - record generation, 1 at creation and incremented on every accepted change
    - created_at: u64 - Unix seconds
    - created_by: String - actor who created the selection
    - record_digest: String - SHA-512 (record domain) of the canonical record without the digest
      and signature
    - signature: String - Ed25519 signature over the record digest
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Record {
    pub(crate) id: String,
    pub(crate) name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) context: Option<String>,
    pub(crate) repository_id: String,
    pub(crate) operation: Operation,
    pub(crate) change_type: Option<String>,
    pub(crate) policy: String,
    pub(crate) language: String,
    pub(crate) origin: Origin,
    pub(crate) current: Current,
    pub(crate) status: Status,
    pub(crate) generation: u64,
    pub(crate) created_at: u64,
    pub(crate) created_by: String,
    pub(crate) record_digest: String,
    pub(crate) signature: String,
}

/** Compute the content digest of normalized selected content
 * Input
    - content: &str - normalized content (see anchors::normalize)
 * Output
    - String of 128 hex characters
*/
pub(crate) fn content_digest(content: &str) -> String {
    digest(CONTENT_DOMAIN, &[content.as_bytes()])
}

/** Compute the origin-binding digest, binding the selection to its repository, checkpoint, blob,
 * original path and range, and baseline content
 * Input
    - repository_id: &str - repository identity digest
    - origin: &Origin - creation facts (its binding_digest field is ignored)
 * Output
    - String of 128 hex characters
*/
pub(crate) fn binding_digest(repository_id: &str, origin: &Origin) -> String {
    let numbers = [
        origin.span.start_line,
        origin.span.end_line,
        origin.span.start_byte,
        origin.span.end_byte,
    ]
    .map(|number| (number as u64).to_be_bytes());
    digest(
        ORIGIN_DOMAIN,
        &[
            repository_id.as_bytes(),
            origin.checkpoint.as_bytes(),
            origin.commit.as_bytes(),
            origin.blob.as_bytes(),
            origin.span.path.as_bytes(),
            &numbers[0],
            &numbers[1],
            &numbers[2],
            &numbers[3],
            origin.content_digest.as_bytes(),
        ],
    )
}

impl Record {
    /** Return the record as JSON without its digest and signature, the form that is digested
     * Input
        - None (uses self)
     * Output
        - Value
    */
    fn unsigned(&self) -> Value {
        let mut value = serde_json::to_value(self).unwrap_or(Value::Null);
        if let Some(object) = value.as_object_mut() {
            object.remove("record_digest");
            object.remove("signature");
        }
        value
    }

    /** Recompute the record digest and sign it
     * Input
        - identity: &SigningIdentity - repository signing key
     * Output
        - None (updates record_digest and signature)
    */
    pub(crate) fn seal(&mut self, identity: &SigningIdentity) {
        self.record_digest = digest_json(RECORD_DOMAIN, &self.unsigned());
        self.signature = identity.sign(RECORD_DOMAIN, self.record_digest.as_bytes());
    }

    /** Verify the record: identifier shape, digest shapes, the origin-binding digest, the record
     * digest, and the signature against any trusted key
     * Input
        - repository_id: &str - identity of the repository being verified
        - trusted_keys: &[String] - trusted public keys
     * Output
        - Result<(), String>
        - Error describing the first failed check
    */
    pub(crate) fn verify(
        &self,
        repository_id: &str,
        trusted_keys: &[String],
    ) -> Result<(), String> {
        if !is_selection_id(&self.id) {
            return Err(format!("record has an invalid identifier '{}'", self.id));
        }
        if self.repository_id != repository_id {
            return Err(format!(
                "selection {} belongs to another repository (copied registry or marker)",
                self.id
            ));
        }
        for (field, value) in [
            ("origin content digest", &self.origin.content_digest),
            ("origin-binding digest", &self.origin.binding_digest),
            ("current content digest", &self.current.content_digest),
            ("record digest", &self.record_digest),
        ] {
            if !is_digest(value) {
                return Err(format!(
                    "selection {} has a malformed {field} (expected 128 hex characters of SHA-512)",
                    self.id
                ));
            }
        }
        if binding_digest(repository_id, &self.origin) != self.origin.binding_digest {
            return Err(format!(
                "selection {} origin was modified (origin-binding digest mismatch)",
                self.id
            ));
        }
        if digest_json(RECORD_DOMAIN, &self.unsigned()) != self.record_digest {
            return Err(format!(
                "selection {} record was modified after signing (record digest mismatch)",
                self.id
            ));
        }
        if trusted_keys.iter().any(|key| {
            verify_signature(
                key,
                RECORD_DOMAIN,
                self.record_digest.as_bytes(),
                &self.signature,
            )
            .is_ok()
        }) {
            Ok(())
        } else {
            Err(format!(
                "selection {} has an invalid or untrusted signature",
                self.id
            ))
        }
    }
}

/** Render .crane/map: a header and one "ID DIGEST" line per selection, sorted by identifier,
 * mapping each identifier to its immutable origin-binding digest
 * Input
    - records: &[Record] - registry records
 * Output
    - String file content
*/
pub(crate) fn render_map(records: &[Record]) -> String {
    let mut lines = records
        .iter()
        .map(|record| format!("{} {}", record.id, record.origin.binding_digest))
        .collect::<Vec<_>>();
    lines.sort();
    let mut text = String::from(
        "# Crane selection map: 8-character selection id -> SHA-512 origin-binding digest\n# Generated by Crane and verified against the signed registry; AI agents cannot modify it.\n",
    );
    for line in lines {
        text.push_str(&line);
        text.push('\n');
    }
    text
}

/** Parse .crane/map into identifier and digest pairs, rejecting malformed lines
 * Input
    - text: &str - file content
 * Output
    - Result<Vec<(String, String)>, String>
    - Error naming the first malformed line
*/
pub(crate) fn parse_map(text: &str) -> Result<Vec<(String, String)>, String> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
        .map(|(number, line)| {
            let mut parts = line.split_whitespace();
            match (parts.next(), parts.next(), parts.next()) {
                (Some(id), Some(digest), None) if is_selection_id(id) && is_digest(digest) => {
                    Ok((id.to_string(), digest.to_string()))
                }
                _ => Err(format!(".crane/map line {} is malformed", number + 1)),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /** Build a sealed record for tests
     * Input
        - identity: &SigningIdentity - signing key
     * Output
        - Record
    */
    fn sample(identity: &SigningIdentity) -> Record {
        let span = Span {
            path: "a.py".into(),
            start_line: 2,
            end_line: 3,
            start_byte: 10,
            end_byte: 30,
        };
        let content = content_digest("x = 1\n");
        let mut origin = Origin {
            checkpoint: "baseline".into(),
            commit: "c".repeat(40),
            blob: "b".repeat(40),
            span: span.clone(),
            content_digest: content.clone(),
            binding_digest: String::new(),
        };
        origin.binding_digest = binding_digest("repo", &origin);
        let mut record = Record {
            id: "K7M2P9RX".into(),
            name: None,
            context: None,
            repository_id: "repo".into(),
            operation: Operation::Preserve,
            change_type: None,
            policy: "default".into(),
            language: "python".into(),
            origin,
            current: Current {
                span,
                content_digest: content,
                resolved_at_generation: 2,
            },
            status: Status::Active,
            generation: 1,
            created_at: 1,
            created_by: "tester".into(),
            record_digest: String::new(),
            signature: String::new(),
        };
        record.seal(identity);
        record
    }

    /** Check that a sealed record verifies and that tampering, foreign repositories, and
     * untrusted keys are all rejected
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn records_detect_tampering_and_untrusted_keys() {
        let identity = SigningIdentity::generate().unwrap();
        let keys = vec![identity.public_hex()];
        let record = sample(&identity);
        assert_eq!(record.record_digest.len(), 128);
        assert!(record.verify("repo", &keys).is_ok());
        assert!(record
            .verify("other", &keys)
            .unwrap_err()
            .contains("another repository"));
        let mut changed = record.clone();
        changed.operation = Operation::Target;
        assert!(changed
            .verify("repo", &keys)
            .unwrap_err()
            .contains("record digest"));
        let mut moved = record.clone();
        moved.origin.span.start_line = 1;
        assert!(moved.verify("repo", &keys).unwrap_err().contains("origin"));
        let stranger = SigningIdentity::generate().unwrap();
        assert!(record
            .verify("repo", &[stranger.public_hex()])
            .unwrap_err()
            .contains("signature"));
    }

    /** Check that the map round-trips and rejects malformed lines
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn map_round_trips() {
        let identity = SigningIdentity::generate().unwrap();
        let record = sample(&identity);
        let parsed = parse_map(&render_map(std::slice::from_ref(&record))).unwrap();
        assert_eq!(
            parsed,
            vec![(record.id.clone(), record.origin.binding_digest)]
        );
        assert!(parse_map("K7M2P9RX abc\n").is_err());
    }
}
