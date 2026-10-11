// Repository administrators. The owner is the key that initialized the repository. The owner signs
// an admin statement, stored in .crane/config.json and therefore covered by the signed manifest,
// that lists the other admins. The registry may then be signed by the owner or by any listed
// admin, so governance no longer depends on one machine. Teammates pin only the owner's key (crane
// init shows its fingerprint and asks), and every admin is trusted through the owner's signature.
//
// Every change to the list raises its epoch. Each machine remembers the highest epoch it verified,
// and rejects an older statement, so a removed admin cannot put back a list that still names
// them. A machine that never saw the newer list cannot tell; reviewing changes to .crane before
// merging covers that case. Without a statement, the repository keeps its original model: only
// the key that signed the current manifest may sign the next one.

use std::fs;

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::state::State;
use crate::platform::files::write_atomic;
use crate::trust::crypto::{canonical_json, verify_signature, SigningIdentity};
use crate::trust::TrustStore;

/** Signature domain of admin statements */
const ADMINS_DOMAIN: &str = "governance/admins/v1";

/** One admin besides the owner
 * Fields
    - public_key: String - Ed25519 public key in hex
    - name: String - label chosen by the owner
    - added_at: u64 - Unix seconds
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Admin {
    pub(crate) public_key: String,
    pub(crate) name: String,
    pub(crate) added_at: u64,
}

/** The owner's signed list of admins
 * Fields
    - owner: String - the owner's public key in hex
    - epoch: u64 - raised by every change to the list
    - admins: Vec<Admin> - admins besides the owner
    - signature: String - the owner's signature over the repository, owner, epoch, and admins
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AdminStatement {
    pub(crate) owner: String,
    pub(crate) epoch: u64,
    pub(crate) admins: Vec<Admin>,
    pub(crate) signature: String,
}

/** Build the signed bytes of a statement
 * Input
    - repository_id: &str - repository identity
    - owner: &str - owner key
    - epoch: u64 - epoch
    - admins: &[Admin] - admins
 * Output
    - Vec<u8>
*/
fn message(repository_id: &str, owner: &str, epoch: u64, admins: &[Admin]) -> Vec<u8> {
    canonical_json(&json!({
        "repository_id": repository_id,
        "owner": owner,
        "epoch": epoch,
        "admins": admins,
    }))
    .into_bytes()
}

impl AdminStatement {
    /** Sign a new statement with the owner's key
     * Input
        - owner: &SigningIdentity - the owner's key
        - repository_id: &str - repository identity
        - epoch: u64 - epoch
        - admins: Vec<Admin> - admins besides the owner
     * Output
        - AdminStatement
    */
    pub(crate) fn sign(
        owner: &SigningIdentity,
        repository_id: &str,
        epoch: u64,
        admins: Vec<Admin>,
    ) -> Self {
        let key = owner.public_hex();
        let signature = owner.sign(ADMINS_DOMAIN, &message(repository_id, &key, epoch, &admins));
        Self {
            owner: key,
            epoch,
            admins,
            signature,
        }
    }

    /** Check the owner's signature
     * Input
        - repository_id: &str - repository identity
     * Output
        - Result<(), String>
    */
    pub(crate) fn verify(&self, repository_id: &str) -> Result<(), String> {
        verify_signature(
            &self.owner,
            ADMINS_DOMAIN,
            &message(repository_id, &self.owner, self.epoch, &self.admins),
            &self.signature,
        )
        .map_err(|_| "the admin list in .crane/config.json is not signed by its owner".to_string())
    }

    /** Return the keys allowed to sign the registry: the owner and every admin
     * Input
        - None (uses self)
     * Output
        - Vec<String>
    */
    pub(crate) fn keys(&self) -> Vec<String> {
        std::iter::once(self.owner.clone())
            .chain(self.admins.iter().map(|admin| admin.public_key.clone()))
            .collect()
    }
}

/** Return the repository owner's key: the statement's owner, else the current manifest signer
 * Input
    - state: &State - governance state
 * Output
    - Option<String>
*/
pub(crate) fn owner(state: &State) -> Option<String> {
    state
        .config
        .admins
        .as_ref()
        .map(|statement| statement.owner.clone())
        .or_else(|| {
            state
                .registry
                .manifest
                .as_ref()
                .map(|manifest| manifest.public_key.clone())
        })
}

/** Return the keys allowed to sign the next manifest
 * Input
    - state: &State - governance state
 * Output
    - Vec<String>
*/
pub(crate) fn signers(state: &State) -> Vec<String> {
    match &state.config.admins {
        Some(statement) => statement.keys(),
        None => owner(state).into_iter().collect(),
    }
}

/** Read the highest admin epoch this machine verified for the repository
 * Input
    - trust: &TrustStore - trust directory
 * Output
    - u64, 0 when none
*/
pub(crate) fn remembered_epoch(trust: &TrustStore) -> u64 {
    fs::read_to_string(trust.repo_dir.join("admins-epoch"))
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(0)
}

/** Remember a verified admin epoch when it is higher than the one remembered
 * Input
    - trust: &TrustStore - trust directory
    - epoch: u64 - verified epoch
 * Output
    - None (a failed write only delays revocation memory to the next verification)
*/
pub(crate) fn remember_epoch(trust: &TrustStore, epoch: u64) {
    if epoch > remembered_epoch(trust) {
        let _ = write_atomic(
            &trust.repo_dir.join("admins-epoch"),
            format!("{epoch}\n").as_bytes(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{Admin, AdminStatement};
    use crate::trust::crypto::SigningIdentity;

    /** Check that a statement verifies, and fails when its admins, epoch, or repository change
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn statements_bind_admins_epoch_and_repository() {
        let owner = SigningIdentity::generate().unwrap();
        let admin = Admin {
            public_key: "ab".repeat(32),
            name: "alice".into(),
            added_at: 1,
        };
        let statement = AdminStatement::sign(&owner, "repo", 2, vec![admin]);
        assert!(statement.verify("repo").is_ok());
        assert!(statement.verify("other").is_err());
        let mut changed = statement.clone();
        changed.epoch = 3;
        assert!(changed.verify("repo").is_err());
        let mut added = statement.clone();
        added.admins[0].public_key = "cd".repeat(32);
        assert!(added.verify("repo").is_err());
        assert_eq!(statement.keys().len(), 2);
    }
}
