// Approvals a person makes on the dashboard after GitHub confirms their role: linking a machine's
// upload key to their GitHub account (they can read the repository), confirming the repository
// owner's key (they administer the repository), and granting autonomy credits to an agent's session
// (they administer the repository). Links and owner keys start on the person's machine ('crane agent
// link', 'crane admin register'), which leaves a request with a one-time code at the service; a
// grant starts on the dashboard. Either way the person signs in with GitHub, and GitHub's answer for
// that repository decides. This is the only place the dashboard writes: it can mark a pending
// request in member_links or owner_keys as confirmed, and record a pending credit grant, which the
// session's machine applies to its own ledger at its next sync. Evidence stays read-only (data.rs).

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

    /** Record a credit grant an admin approved; the session's machine applies it to its own ledger
     * at its next sync and reports back
     * Input
        - grant: &Grant - what was granted, to which session and machine
        - login: &str - the admin's GitHub login
        - access: Access - their access to the repository, from GitHub
     * Output
        - Result<String, String> what was recorded, for the person
    */
    pub(crate) fn grant(
        &self,
        grant: &Grant,
        login: &str,
        access: Access,
    ) -> Result<String, String> {
        if !access.admin {
            return Err(format!("GitHub does not show @{login} as an admin of {}; only admins and owners can grant credits", grant.remote));
        }
        let now = now_millis();
        let grant_id = format!(
            "grt_{}",
            &digest(
                "dashboard/grant-id/v1",
                &[
                    grant.repository.as_bytes(),
                    grant.session.as_bytes(),
                    login.as_bytes(),
                    now.to_string().as_bytes()
                ]
            )[..24]
        );
        self.database
            .collection::<Document>("credit_grants")
            .insert_one(doc! {
                "grant_id": &grant_id,
                "repository_id": &grant.repository,
                "installation": &grant.installation,
                "session": &grant.session,
                "agent_id": &grant.agent,
                "amount": grant.amount as i64,
                "reason": &grant.reason,
                "approved_by": login,
                "approved_at": now as i64,
                "expires_at": (now + GRANT_LIFETIME_MILLIS) as i64,
                "status": "PENDING",
            })
            .run()
            .map_err(io_error)?;
        Ok(format!(
            "{} credits for session {} are approved by @{login} ({grant_id}). The agent's machine applies them at its next sync, within about a minute while the agent is active.",
            grant.amount, grant.session
        ))
    }
}

/** How long a grant waits for its machine before it lapses */
const GRANT_LIFETIME_MILLIS: u64 = 7 * 24 * 60 * 60 * 1000;

/** A credit grant asked for on the dashboard
 * Fields
    - repository: String - repository identity
    - remote: String - its verified remote
    - session: String - the agent session to credit
    - installation: String - the machine the session runs on
    - agent: String - the agent identity
    - amount: u64 - credits, 1 to 1000
    - reason: String - why, shown in the ledger
*/
pub(crate) struct Grant {
    pub(crate) repository: String,
    pub(crate) remote: String,
    pub(crate) session: String,
    pub(crate) installation: String,
    pub(crate) agent: String,
    pub(crate) amount: u64,
    pub(crate) reason: String,
}
