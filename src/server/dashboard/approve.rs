// Approvals a person makes on the dashboard after GitHub confirms their role: linking a machine's
// upload key to their GitHub account (they can read the repository), confirming the repository
// owner's key (they administer the repository), and requesting Brick and flow changes from the
// Flows section (a writer for drafts; a code owner of the signed file, else an admin, for signed
// changes). Links and owner keys start on the person's machine ('crane agent link', 'crane admin
// register'), which leaves a request with a one-time code at the service; a change request starts
// on the dashboard. Either way the person signs in with GitHub, and GitHub's answer for that
// repository decides. Requests are applied on a machine, never here.

use mongodb::bson::{self, doc, Document};
use mongodb::sync::Database;
use serde_json::Value;

use super::auth::{Access, Intent, Ownership};
use crate::platform::{io_error, now_millis};
use crate::telemetry::ingest::CODE_DOMAIN;
use crate::trust::crypto::digest;
use crate::trust::crypto::random_id;

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

/** How many Brick requests one person may have waiting per repository */
const BRICK_OPEN_LIMIT: u64 = 50;

/** How long a Brick request waits for GitHub to confirm the person's role */
const BRICK_STAGE_MILLIS: u64 = 10 * 60 * 1000;

/** How long a confirmed Brick request waits for a machine before it lapses */
pub(crate) const BRICK_REQUEST_MILLIS: u64 = 14 * 24 * 60 * 60 * 1000;

impl Approvals {
    /** Keep a Brick change request until GitHub confirms the person's role at sign-in: checked
     * with the rules the machine checks again (brick/requests.rs), and unconfirmed it is only a
     * staged row that no machine ever reads, removed within the hour
     * Input
        - repository: &str - repository identity
        - login: &str - the signed-in person
        - op: &str - operation
        - params: &Value - its parameters
        - reason: &str - why
     * Output
        - Result<(String, &'static str), String> the request id and the role it needs
    */
    pub(crate) fn stage_brick_request(
        &self,
        repository: &str,
        login: &str,
        op: &str,
        params: &Value,
        reason: &str,
    ) -> Result<(String, &'static str), String> {
        let kept = crate::brick::requests::check(op, params, reason)?;
        let open = self
            .database
            .collection::<Document>("brick_requests")
            .count_documents(doc! {"repository_id": repository, "requested_by": login, "status": {"$in": ["STAGED", "PENDING"]}})
            .run()
            .map_err(io_error)?;
        if open >= BRICK_OPEN_LIMIT {
            return Err(format!("you have {open} Brick requests waiting for this repository; apply, dismiss, or withdraw some first"));
        }
        let role = crate::brick::requests::role(op)
            .ok_or_else(|| format!("'{op}' cannot be requested"))?
            .name();
        let now = now_millis();
        let request_id = format!("brq_{}{}", random_id()?, random_id()?);
        self.database
            .collection::<Document>("brick_requests")
            .insert_one(doc! {
                "request_id": &request_id,
                "repository_id": repository,
                "op": op,
                "params": serde_json::to_string(&kept).map_err(|error| error.to_string())?,
                "reason": crate::telemetry::redact::redact(reason.trim()).0,
                "role": role,
                "requested_by": login,
                "requested_at": now as i64,
                "status": "STAGED",
                "purge_at": bson::DateTime::from_millis((now + 60 * 60 * 1000) as i64),
            })
            .run()
            .map_err(io_error)?;
        Ok((request_id, role))
    }

    /** Confirm a staged Brick request with GitHub's answer for the repository: writers (push,
     * maintain, admin) may request drafts; signed changes need an admin. Confirmed, it waits for a
     * person on a machine to apply it; refused, it is kept as refused so the page can say why
     * Input
        - request_id: &str - the staged request
        - repository: &str - repository identity
        - remote: &str - its verified remote (for the message)
        - login: &str - who signed in (must be who staged it)
        - access: Access - their access to the repository, from GitHub
     * Output
        - Result<String, String> the message for the person
    */
    pub(crate) fn confirm_brick_request(
        &self,
        request_id: &str,
        repository: &str,
        remote: &str,
        login: &str,
        access: Access,
        owners: Option<&Ownership>,
    ) -> Result<String, String> {
        let now = now_millis();
        let collection = self.database.collection::<Document>("brick_requests");
        let staged = collection
            .find_one(doc! {"request_id": request_id, "repository_id": repository, "requested_by": login, "status": "STAGED",
                "requested_at": {"$gt": now.saturating_sub(BRICK_STAGE_MILLIS) as i64}})
            .run()
            .map_err(io_error)?
            .ok_or("that request was not found or waited too long for sign-in; make it again")?;
        let role = staged.get_str("role").unwrap_or("admin");
        let owned = owners.is_some_and(|owners| owners.owned);
        let allowed = match (role, owners) {
            // A code owner of the signed store may draft too
            ("writer", _) => access.write || owned,
            // CODEOWNERS cover the signed store: only its owners request signed changes
            (_, Some(_)) => owned,
            (_, None) => access.admin,
        };
        let kept = now + super::BRICK_REQUEST_KEPT_MILLIS;
        if !allowed {
            let detail = match (role, owners) {
                ("writer", _) => format!("GitHub does not show @{login} with write access to {remote}; drafts need push, maintain, or admin"),
                (_, Some(owners)) if owners.owners.is_empty() => format!("{} gives {} no owner (rule '{}'), so no one may request this signed change from the dashboard; add an owner, or apply the change on a machine", owners.file, owners.path, owners.rule),
                (_, Some(owners)) => format!("@{login} does not own {} by {} (rule '{}': {}); this signed change needs one of its code owners", owners.path, owners.file, owners.rule, owners.owners.iter().map(|owner| format!("@{owner}")).collect::<Vec<_>>().join(", ")),
                (_, None) => format!("GitHub does not show @{login} as an admin of {remote}, and no CODEOWNERS line covers {}; this signed change needs an admin or owner", super::auth::signed_path(staged.get_str("op").unwrap_or_default())),
            };
            collection
                .update_one(
                    doc! {"request_id": request_id, "status": "STAGED"},
                    doc! {"$set": {"status": "REFUSED", "detail": &detail, "settled_at": now as i64, "purge_at": bson::DateTime::from_millis(kept as i64)}},
                )
                .run()
                .map_err(io_error)?;
            return Err(detail);
        }
        collection
            .update_one(
                doc! {"request_id": request_id, "status": "STAGED"},
                doc! {"$set": {"status": "PENDING", "confirmed_at": now as i64, "expires_at": (now + BRICK_REQUEST_MILLIS) as i64,
                    "purge_at": bson::DateTime::from_millis((kept + BRICK_REQUEST_MILLIS) as i64)}},
            )
            .run()
            .map_err(io_error)?;
        Ok(format!(
            "Request {request_id} is waiting for a machine: a person there applies it with one click in Landscape or 'crane brick inbox apply {request_id}', and Crane checks{} it there.",
            if role == "writer" { "" } else { " and signs" }
        ))
    }

    /** The operation of a request still staged for its author's sign-in
     * Input
        - request_id: &str - the request
        - repository: &str - repository identity
        - login: &str - the signed-in person
     * Output
        - Option<String>
    */
    pub(crate) fn staged_brick_op(
        &self,
        request_id: &str,
        repository: &str,
        login: &str,
    ) -> Option<String> {
        self.database
            .collection::<Document>("brick_requests")
            .find_one(doc! {"request_id": request_id, "repository_id": repository, "requested_by": login, "status": "STAGED"})
            .run()
            .ok()
            .flatten()
            .and_then(|document| document.get_str("op").ok().map(str::to_string))
    }

    /** Withdraw a Brick request that is still staged or waiting (only its author may)
     * Input
        - repository: &str - repository identity
        - request_id: &str - the request
        - login: &str - the signed-in person
     * Output
        - Result<(), String>
    */
    pub(crate) fn cancel_brick_request(
        &self,
        repository: &str,
        request_id: &str,
        login: &str,
    ) -> Result<(), String> {
        let now = now_millis();
        let changed = self
            .database
            .collection::<Document>("brick_requests")
            .update_one(
                doc! {"request_id": request_id, "repository_id": repository, "requested_by": login, "status": {"$in": ["STAGED", "PENDING"]}},
                doc! {"$set": {"status": "CANCELLED", "settled_at": now as i64, "detail": format!("withdrawn by @{login}"),
                    "purge_at": bson::DateTime::from_millis((now + super::BRICK_REQUEST_KEPT_MILLIS) as i64)}},
            )
            .run()
            .map_err(io_error)?;
        if changed.modified_count == 0 {
            return Err("only a waiting request you made can be withdrawn".into());
        }
        Ok(())
    }
}
