// Approvals a person makes on the dashboard after GitHub confirms their role: linking a machine's
// upload key to their GitHub account (they can read the repository) and confirming the repository
// owner's key (they administer the repository). Each starts on the person's machine ('crane agent
// link', 'crane admin register'), which leaves a request with a one-time code at the service; the
// person opens /auth/approve with the code, signs in with GitHub, and GitHub's answer for that
// repository decides. This is the only place the dashboard writes, and it can only mark a pending
// request in member_links or owner_keys as confirmed: evidence stays read-only (data.rs).

use mongodb::bson::{doc, Document};
use mongodb::sync::Database;

use super::auth::{Access, Intent};
use crate::platform::{io_error, now_millis};
use crate::telemetry::ingest::CODE_DOMAIN;
use crate::trust::crypto::digest;

/** The dashboard's narrow write access
 * Fields
    - database: Database - MongoDB (private, so only the methods below can write)
*/
pub(crate) struct Approvals {
    database: Database,
}

impl Approvals {
    /** Wrap a database
     * Input
        - database: Database - MongoDB
     * Output
        - Approvals
    */
    pub(crate) fn new(database: Database) -> Self {
        Self { database }
    }

    /** Find the pending request of an intent: unused, unexpired, and of the right kind
     * Input
        - intent: &Intent - what the person asked to approve
     * Output
        - Result<(&'static str, Document), String> collection and request
        - Error if the action is unknown or the code is unknown, used, or expired
    */
    pub(crate) fn pending(&self, intent: &Intent) -> Result<(&'static str, Document), String> {
        let collection = match intent.action.as_str() {
            "link" => "member_links",
            "owner" => "owner_keys",
            _ => return Err("unknown approval".into()),
        };
        self.database
            .collection::<Document>(collection)
            .find_one(doc! {
                "code_digest": digest(CODE_DOMAIN, &[intent.code.as_bytes()]),
                "expires_at": {"$gt": now_millis() as i64},
                "confirmed_by": {"$exists": false},
            })
            .run()
            .map_err(io_error)?
            .map(|document| (collection, document))
            .ok_or_else(|| {
                "the code is unknown, already used, or expired; run the command on your machine again".into()
            })
    }

    /** Confirm a pending request when GitHub's answer allows it: a link needs read access to the
     * repository, an owner key needs admin access
     * Input
        - intent: &Intent - what the person asked to approve
        - login: &str - the person's GitHub login
        - access: Access - their access to the request's repository, from GitHub
     * Output
        - Result<String, String> what was confirmed, for the person
    */
    pub(crate) fn confirm(
        &self,
        intent: &Intent,
        login: &str,
        access: Access,
    ) -> Result<String, String> {
        let (collection, pending) = self.pending(intent)?;
        let remote = pending.get_str("remote").unwrap_or_default().to_string();
        let allowed = if collection == "owner_keys" {
            access.admin
        } else {
            access.read
        };
        if !allowed {
            return Err(format!(
                "GitHub does not show @{login} as {} of {remote}; nothing was confirmed",
                if collection == "owner_keys" {
                    "an admin"
                } else {
                    "having access"
                }
            ));
        }
        self.database
            .collection::<Document>(collection)
            .update_one(
                doc! {"_id": pending.get("_id").cloned().unwrap_or_default(), "confirmed_by": {"$exists": false}},
                doc! {"$set": {"confirmed_by": login, "confirmed_at": now_millis() as i64}},
            )
            .run()
            .map_err(io_error)?;
        let short = |key: &str| key.chars().take(16).collect::<String>();
        Ok(if collection == "owner_keys" {
            format!(
                "The owner key {}… of {remote} is confirmed by @{login}, an admin of the repository. Teammates joining with crane init see this confirmation.",
                short(pending.get_str("owner_key").unwrap_or_default())
            )
        } else {
            format!(
                "The machine {}… is linked to @{login} for {remote}; its activity is now shown as verified.",
                short(pending.get_str("installation").unwrap_or_default())
            )
        })
    }
}
