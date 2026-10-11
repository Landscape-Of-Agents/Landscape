// The governance state and its signed manifest. Every Crane command that changes governance
// (init, checkpoint, protect, target, defaults, policy moves, context bindings) goes through
// commit(), which verifies the current state first, applies the change, appends an audit entry,
// and signs a new manifest whose generation is one higher. The manifest covers the registry
// records, checkpoints, configuration, context bindings, and audit chain by digest, and this
// machine's rollback anchor (in the trust directory) records the latest generation seen, so
// modification, deletion, rollback, and forks of .crane are all detected by integrity().

use std::fs;
use std::io::ErrorKind;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::admins;
use super::audit::{self, Entry};
use super::checkpoints::CheckpointLog;
use super::config::Config;
use super::context::Bindings;
use super::workspace::Workspace;
use super::{Finding, Severity};
use crate::platform::files::{append_line, read_lines, write_atomic, Lock};
use crate::platform::{actor, git, io_error, now_unix};
use crate::selection::registry::{parse_map, render_map, Record, REGISTRY_FORMAT};
use crate::trust::crypto::{digest_json, verify_signature, SigningIdentity};
use crate::trust::{require_human, TrustStore};

/** Digest and signature domain of the manifest */
const MANIFEST_DOMAIN: &str = "registry/manifest/v1";

/** The signed manifest heading .crane/registry.json
 * Fields
    - format: u64 - registry layout version
    - repository_id: String - repository identity digest
    - public_key: String - Ed25519 public key that signed this generation
    - generation: u64 - incremented by every governance change
    - previous_head: String - head of the previous generation
    - records_digest: String - digest of every record's id and record digest
    - checkpoints_digest: String - digest of .crane/checkpoints.json
    - config_digest: String - digest of .crane/config.json
    - context_digest: String - digest of .crane/context.json
    - audit_head: String - head of .crane/audit.jsonl
    - updated_at: u64 - Unix seconds
    - updated_by: String - actor
    - head: String - SHA-512 of this manifest without head and signature
    - signature: String - Ed25519 signature over head
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Manifest {
    pub(crate) format: u64,
    pub(crate) repository_id: String,
    pub(crate) public_key: String,
    pub(crate) generation: u64,
    pub(crate) previous_head: String,
    pub(crate) records_digest: String,
    pub(crate) checkpoints_digest: String,
    pub(crate) config_digest: String,
    pub(crate) context_digest: String,
    pub(crate) audit_head: String,
    pub(crate) updated_at: u64,
    pub(crate) updated_by: String,
    pub(crate) head: String,
    pub(crate) signature: String,
}

impl Manifest {
    /** Compute the manifest head from every field but head and signature
     * Input
        - None (uses self)
     * Output
        - String of 128 hex characters
    */
    fn compute_head(&self) -> String {
        let mut value = serde_json::to_value(self).unwrap_or(Value::Null);
        if let Some(object) = value.as_object_mut() {
            object.remove("head");
            object.remove("signature");
        }
        digest_json(MANIFEST_DOMAIN, &value)
    }
}

/** The content of .crane/registry.json
 * Fields
    - manifest: Option<Manifest> - signed manifest, None before init completes
    - selections: Vec<Record> - selection records
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RegistryFile {
    pub(crate) manifest: Option<Manifest>,
    pub(crate) selections: Vec<Record>,
}

/** Everything the manifest covers, loaded from .crane
 * Fields
    - config: Config - .crane/config.json
    - checkpoints: CheckpointLog - .crane/checkpoints.json
    - context: Bindings - .crane/context.json
    - registry: RegistryFile - .crane/registry.json
    - audit: Vec<Entry> - .crane/audit.jsonl
*/
#[derive(Debug, Clone, Default)]
pub(crate) struct State {
    pub(crate) config: Config,
    pub(crate) checkpoints: CheckpointLog,
    pub(crate) context: Bindings,
    pub(crate) registry: RegistryFile,
    pub(crate) audit: Vec<Entry>,
}

/** Read and parse a JSON file of .crane, using the default value when the file is missing
 * Input
    - workspace: &Workspace - repository
    - name: &str - file name inside .crane
 * Output
    - Result<T, String>
    - Error if the file exists but cannot be read or parsed
*/
fn read_json<T: for<'de> Deserialize<'de> + Default>(
    workspace: &Workspace,
    name: &str,
) -> Result<T, String> {
    match fs::read_to_string(workspace.file(name)) {
        Ok(text) if has_conflict_markers(&text) => Err(conflicted(name)),
        Ok(text) => serde_json::from_str(&text).map_err(|error| {
            format!(".crane/{name} is malformed ({error}); it may have been tampered with")
        }),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(T::default()),
        Err(error) => Err(format!("could not read .crane/{name}: {error}")),
    }
}

/** Report whether text contains Git merge conflict markers
 * Input
    - text: &str - file content
 * Output
    - bool
*/
fn has_conflict_markers(text: &str) -> bool {
    text.lines()
        .any(|line| line.starts_with("<<<<<<< ") || line.starts_with(">>>>>>> "))
}

/** Explain how to resolve a merge conflict in .crane: two admins changed the rules on different
 * branches, so each side has its own signed generation and only one can be kept
 * Input
    - name: &str - file name inside .crane
 * Output
    - String
*/
fn conflicted(name: &str) -> String {
    format!(
        ".crane/{name} has unresolved merge conflicts: two admins changed the rules on different branches. Keep one side for the whole directory ('git checkout --theirs .crane' or '--ours'), commit, run 'crane init' to accept that signed state, then re-apply your change with crane so it is signed again"
    )
}

impl State {
    /** Load the governance state from .crane
     * Input
        - workspace: &Workspace - repository
     * Output
        - Result<State, String>
        - Error if a file is unreadable or malformed
    */
    pub(crate) fn load(workspace: &Workspace) -> Result<Self, String> {
        for name in ["audit.jsonl", "map"] {
            if fs::read_to_string(workspace.file(name))
                .is_ok_and(|text| has_conflict_markers(&text))
            {
                return Err(conflicted(name));
            }
        }
        let audit = read_lines(&workspace.file("audit.jsonl"))?
            .into_iter()
            .map(|value| {
                serde_json::from_value(value)
                    .map_err(|error| format!(".crane/audit.jsonl has a malformed entry ({error})"))
            })
            .collect::<Result<Vec<Entry>, String>>()?;
        Ok(Self {
            config: read_json(workspace, "config.json")?,
            checkpoints: read_json(workspace, "checkpoints.json")?,
            context: read_json(workspace, "context.json")?,
            registry: read_json(workspace, "registry.json")?,
            audit,
        })
    }

    /** Find a selection record by identifier
     * Input
        - id: &str - selection identifier
     * Output
        - Option<&Record>
    */
    pub(crate) fn record(&self, id: &str) -> Option<&Record> {
        self.registry
            .selections
            .iter()
            .find(|record| record.id == id)
    }

    /** Return the registry generation, 0 before init
     * Input
        - None (uses self)
     * Output
        - u64
    */
    pub(crate) fn generation(&self) -> u64 {
        self.registry
            .manifest
            .as_ref()
            .map_or(0, |manifest| manifest.generation)
    }

    /** Compute the digests the manifest covers
     * Input
        - None (uses self)
     * Output
        - [String; 5] records, checkpoints, config, context digests and the audit head
    */
    fn covered(&self) -> [String; 5] {
        let mut records = self
            .registry
            .selections
            .iter()
            .map(|record| json!([record.id, record.record_digest]))
            .collect::<Vec<_>>();
        records.sort_by_key(|pair| pair[0].as_str().unwrap_or_default().to_string());
        [
            digest_json("registry/records/v1", &Value::Array(records)),
            digest_json(
                "governance/checkpoints/v1",
                &serde_json::to_value(&self.checkpoints).unwrap_or(Value::Null),
            ),
            digest_json(
                "governance/config/v1",
                &serde_json::to_value(&self.config).unwrap_or(Value::Null),
            ),
            digest_json(
                "governance/context/v1",
                &serde_json::to_value(&self.context).unwrap_or(Value::Null),
            ),
            audit::head(&self.audit),
        ]
    }
}

/** Build a critical integrity finding
 * Input
    - code: &str - machine-readable code
    - message: String - explanation
 * Output
    - Finding
*/
fn critical(code: &str, message: String) -> Finding {
    Finding::new(code, Severity::Critical, message)
}

/** Verify the governance state against its signed manifest, the trusted keys, and this machine's
 * rollback anchor: manifest presence, repository binding, trusted signer, head and signature,
 * the digests of every covered file, every record, the map, the checkpoint and audit chains,
 * checkpoint commits, and rollback or fork against the anchor
 * Input
    - workspace: &Workspace - repository
    - state: &State - loaded state
    - trust: &TrustStore - trust directory
 * Output
    - Vec<Finding>, empty when everything verifies (an informational finding is added the first
      time this machine sees the repository)
*/
pub(crate) fn integrity(workspace: &Workspace, state: &State, trust: &TrustStore) -> Vec<Finding> {
    let mut findings = Vec::new();
    let Some(manifest) = &state.registry.manifest else {
        findings.push(critical(
            "registry_missing",
            ".crane/registry.json has no signed manifest (deleted, replaced, or never initialized); run 'crane init'".into(),
        ));
        return findings;
    };
    if manifest.repository_id != workspace.repository_id {
        findings.push(critical(
            "registry_wrong_repository",
            "the registry is bound to another repository (copied .crane or changed origin remote)"
                .into(),
        ));
    }
    let pinned = match trust.trusted_public_keys() {
        Ok(keys) => keys,
        Err(error) => {
            findings.push(critical("trust_unavailable", error));
            return findings;
        }
    };
    let short = |key: &str| key[..key.len().min(16)].to_string();
    // With an admin list, the signers are the owner and the admins, trusted through the owner
    let mut keys = pinned.clone();
    let mut verified_epoch = None;
    if let Some(statement) = &state.config.admins {
        let remembered = admins::remembered_epoch(trust);
        if !pinned.contains(&statement.owner) {
            findings.push(critical(
                "untrusted_owner",
                format!(
                    "this machine does not trust the repository owner's key ({}...); run 'crane init' to review its fingerprint and trust it",
                    short(&statement.owner)
                ),
            ));
        } else if let Err(error) = statement.verify(&workspace.repository_id) {
            findings.push(critical("admins_tampered", error));
        } else if statement.epoch < remembered {
            findings.push(critical(
                "admins_rollback",
                format!(
                    "the admin list was rolled back to version {} after this machine verified version {remembered} (a removed admin may have restored an old list)",
                    statement.epoch
                ),
            ));
        } else {
            keys = statement.keys();
            verified_epoch = Some(statement.epoch);
        }
    }
    if pinned.is_empty() {
        findings.push(critical(
            "no_trusted_key",
            "this machine holds no trusted key for the repository, so signatures cannot be verified; run 'crane init' to review the owner's key fingerprint and trust it (or set CRANE_TRUSTED_PUBLIC_KEYS on CI)".into(),
        ));
    } else if !keys.contains(&manifest.public_key) {
        findings.push(critical(
            "untrusted_signer",
            format!(
                "the registry is signed by a key that is not trusted or not an admin ({}...); run 'crane init' to review it",
                short(&manifest.public_key)
            ),
        ));
    }
    if manifest.compute_head() != manifest.head {
        findings.push(critical(
            "manifest_tampered",
            "the registry manifest was modified after signing".into(),
        ));
    } else if verify_signature(
        &manifest.public_key,
        MANIFEST_DOMAIN,
        manifest.head.as_bytes(),
        &manifest.signature,
    )
    .is_err()
    {
        findings.push(critical(
            "manifest_signature_invalid",
            "the registry manifest signature does not verify".into(),
        ));
    }
    let covered = state.covered();
    for ((name, expected), actual) in [
        ("registry selections", &manifest.records_digest),
        (".crane/checkpoints.json", &manifest.checkpoints_digest),
        (".crane/config.json", &manifest.config_digest),
        (".crane/context.json", &manifest.context_digest),
        (".crane/audit.jsonl", &manifest.audit_head),
    ]
    .into_iter()
    .zip(covered.iter())
    {
        if expected != actual {
            findings.push(critical(
                "governance_file_tampered",
                format!("{name} changed outside Crane (digest does not match the signed manifest)"),
            ));
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    for record in &state.registry.selections {
        if !seen.insert(record.id.as_str()) {
            findings.push(
                critical(
                    "selection_duplicated",
                    format!("selection {} appears twice in the registry", record.id),
                )
                .with_selection(&record.id),
            );
        }
        if let Err(error) = record.verify(&workspace.repository_id, &keys) {
            findings.push(critical("selection_record_invalid", error).with_selection(&record.id));
        }
    }
    match fs::read_to_string(workspace.file("map")) {
        Err(_) => findings.push(critical("map_missing", ".crane/map is missing".into())),
        Ok(text) => match parse_map(&text) {
            Err(error) => findings.push(critical("map_tampered", error)),
            Ok(mut pairs) => {
                pairs.sort();
                let mut expected = state
                    .registry
                    .selections
                    .iter()
                    .map(|record| (record.id.clone(), record.origin.binding_digest.clone()))
                    .collect::<Vec<_>>();
                expected.sort();
                if pairs != expected {
                    findings.push(critical(
                        "map_tampered",
                        ".crane/map does not match the signed registry (identifier or digest changed, added, or removed)".into(),
                    ));
                }
            }
        },
    }
    if let Err(error) = state.checkpoints.verify_chain() {
        findings.push(critical("checkpoints_tampered", error));
    }
    for checkpoint in &state.checkpoints.checkpoints {
        if !git::commit_exists(&workspace.root, &checkpoint.commit) {
            findings.push(Finding::new(
                "checkpoint_commit_missing",
                Severity::High,
                format!(
                    "checkpoint '{}' commit {} is not available locally; fetch it",
                    checkpoint.name, checkpoint.commit
                ),
            ));
        }
    }
    if let Err(error) = audit::verify(&state.audit) {
        findings.push(critical("audit_tampered", error));
    }
    if let Some(epoch) = verified_epoch {
        if !findings.iter().any(Finding::blocking) {
            admins::remember_epoch(trust, epoch);
        }
    }
    match trust.anchor() {
        Err(error) => findings.push(critical("anchor_unreadable", error)),
        Ok(None) => findings.push(Finding::new(
            "anchor_first_use",
            Severity::Low,
            "this machine has no rollback anchor for the repository yet; rollback detection starts after the next governance change made here".into(),
        )),
        Ok(Some(anchor)) => {
            if let Err(error) = trust.verify_anchor(&anchor) {
                findings.push(critical("anchor_invalid", error));
            } else if manifest.generation < anchor.generation {
                findings.push(critical(
                    "registry_rollback",
                    format!(
                        "the registry was rolled back: generation {} is older than generation {} already seen on this machine",
                        manifest.generation, anchor.generation
                    ),
                ));
            } else if manifest.generation == anchor.generation && manifest.head != anchor.head {
                findings.push(critical(
                    "registry_fork",
                    format!(
                        "generation {} differs from the generation {} this machine signed (replaced registry)",
                        manifest.generation, anchor.generation
                    ),
                ));
            }
        }
    }
    findings
}

/** Apply a governance change: refuse agents, take the governance lock, load the state, refuse to
 * sign over a state that fails integrity (except the very first commit), apply the change, append
 * an audit entry, sign a new manifest one generation higher, write every covered file, and move
 * this machine's rollback anchor
 * Input
    - workspace: &Workspace - repository
    - command: &str - command name for the agent refusal message
    - action: &str - audit action, such as selection.created
    - detail: Value - audit detail (never source content)
    - mutate: impl FnOnce(&mut State, u64) -> Result<(), String> - the change, given the new
      generation number
 * Output
    - Result<State, String> the committed state
    - Error if run by an agent, the state does not verify, the change fails, or a write fails
*/
pub(crate) fn commit(
    workspace: &Workspace,
    command: &str,
    action: &str,
    detail: Value,
    mutate: impl FnOnce(&mut State, u64) -> Result<(), String>,
) -> Result<State, String> {
    require_human(command)?;
    let trust = workspace.trust()?;
    let _lock = Lock::acquire(&trust.repo_dir.join("governance.lock"))?;
    let mut state = State::load(workspace)?;
    let first = state.registry.manifest.is_none();
    let identity: SigningIdentity = if first {
        trust.signing_key_or_create()?
    } else {
        trust.require_signing_key()?
    };
    if !first && !admins::signers(&state).contains(&identity.public_hex()) {
        return Err(format!(
            "this machine's key is not an admin of the repository; ask the owner to run 'crane admin add {} name YOUR_NAME'",
            identity.public_hex()
        ));
    }
    if !first {
        let blocking = integrity(workspace, &state, &trust)
            .into_iter()
            .filter(|finding| finding.severity >= Severity::High)
            .map(|finding| finding.message)
            .collect::<Vec<_>>();
        if !blocking.is_empty() {
            return Err(format!(
                "refusing to sign over a governance state that fails verification: {}; run 'crane validate'",
                blocking.join("; ")
            ));
        }
    }
    let generation = state.generation() + 1;
    let previous_head = state.registry.manifest.as_ref().map_or_else(
        || audit::GENESIS.to_string(),
        |manifest| manifest.head.clone(),
    );
    mutate(&mut state, generation)?;
    for record in &mut state.registry.selections {
        record.seal(&identity);
    }
    let who = actor();
    let entry = audit::next(&state.audit, now_unix(), &who, action, detail);
    state.audit.push(entry.clone());
    let [records_digest, checkpoints_digest, config_digest, context_digest, audit_head] =
        state.covered();
    let mut manifest = Manifest {
        format: REGISTRY_FORMAT,
        repository_id: workspace.repository_id.clone(),
        public_key: identity.public_hex(),
        generation,
        previous_head,
        records_digest,
        checkpoints_digest,
        config_digest,
        context_digest,
        audit_head,
        updated_at: now_unix(),
        updated_by: who,
        head: String::new(),
        signature: String::new(),
    };
    manifest.head = manifest.compute_head();
    manifest.signature = identity.sign(MANIFEST_DOMAIN, manifest.head.as_bytes());
    state.registry.manifest = Some(manifest.clone());
    let pretty = |value: Value| serde_json::to_string_pretty(&value).map(|text| text + "\n");
    for (name, value) in [
        ("config.json", serde_json::to_value(&state.config)),
        ("checkpoints.json", serde_json::to_value(&state.checkpoints)),
        ("context.json", serde_json::to_value(&state.context)),
        ("registry.json", serde_json::to_value(&state.registry)),
    ] {
        let text = pretty(value.map_err(io_error)?).map_err(io_error)?;
        write_atomic(&workspace.file(name), text.as_bytes())?;
    }
    write_atomic(
        &workspace.file("map"),
        render_map(&state.registry.selections).as_bytes(),
    )?;
    append_line(
        &workspace.file("audit.jsonl"),
        &serde_json::to_string(&entry).map_err(io_error)?,
    )?;
    trust.set_anchor(&identity, generation, &manifest.head)?;
    Ok(state)
}
