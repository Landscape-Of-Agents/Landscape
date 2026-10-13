// What the service sees in an upload and records as events (DASHBOARD.md): a machine seen for the
// first time (F02.9), its remote proven or not (F09.5), the upload itself (F09.1 to F09.4, from the
// summary on the last batch), a broken evidence chain (F09.6), and the changes between this
// machine's status reports: the report itself (F28.1), the rules' integrity failing, warning, or
// restored (F12), blocks that moved, broke, or found candidates (F18), and context files whose
// state changed (F20). The previous records are read before the upload is stored.

use mongodb::bson::{doc, Document};
use mongodb::sync::Database;
use serde_json::Value;

use super::service_events::{self, ServiceLog};
use crate::telemetry::ingest::Batch;
use crate::telemetry::spec::Spec;

/** What the service held before an upload
 * Fields
    - known: bool - the machine had uploaded for this repository before
    - verified: Option<bool> - whether its remote was proven
    - status: Option<Value> - its previous status report
*/
pub(super) struct Before {
    known: bool,
    verified: Option<bool>,
    status: Option<Value>,
}

/** Read what the service holds for a machine and repository
 * Input
    - database: &Database - database
    - repository: &str - repository identity
    - installation: &str - machine
 * Output
    - Before
*/
pub(super) fn before(database: &Database, repository: &str, installation: &str) -> Before {
    let scope = doc! {"repository_id": repository, "installation": installation};
    let find = |collection: &str| {
        database
            .collection::<Document>(collection)
            .find_one(scope.clone())
            .run()
            .ok()
            .flatten()
    };
    let member = find("repositories");
    Before {
        known: member.is_some(),
        verified: member
            .as_ref()
            .and_then(|member| member.get_bool("verified").ok()),
        status: find("repository_status").and_then(|status| serde_json::to_value(status).ok()),
    }
}

/** Start an upload event for a machine
 * Input
    - name: &str - event name
    - flow: &str - flow
    - action_type: &str - action type
    - batch: &Batch - the batch
 * Output
    - Spec
*/
fn machine_event(name: &str, flow: &str, action_type: &str, batch: &Batch) -> Spec {
    let mut spec = service_events::event(name, flow, action_type, "SERVICE");
    spec.set("action", "action_user", &batch.actor)
        .set("repository", "repo_remote", &batch.remote)
        .set(
            "repository",
            "registry_generation",
            batch.registry_generation,
        )
        .set("bindings", "third_party", ["FOXX_SERVICE", "MONGODB"]);
    spec
}

/** Record what an accepted upload shows
 * Input
    - log: &ServiceLog - event log
    - repository: &str - repository identity
    - installation: &str - machine
    - batch: &Batch - the batch
    - before: Before - what the service held before it
    - stored: &Value - the store's answer (duplicates, chain_problems)
    - verified: bool - the batch proved its remote
 * Output
    - None
*/
pub(super) fn batch(
    log: &ServiceLog,
    repository: &str,
    installation: &str,
    batch: &Batch,
    before: Before,
    stored: &Value,
    verified: bool,
) {
    let record = |spec: Spec| log.record(spec, Some(repository), Some(installation));
    if !before.known {
        let mut spec = machine_event("MACHINE_FIRST_SEEN", "F02", "FIRST_UPLOAD", batch);
        spec.respond("OK", "NA", "First upload from this machine.")
            .severity("INFO", "FIXED", "NONE")
            .set("assessment", "assessment_checks", ["REMOTE_PROVEN"])
            .set("other", "machine_key", installation)
            .set("other", "linked", false);
        record(spec);
    }
    if batch.complete && before.verified != Some(verified) {
        let mut spec = machine_event("REMOTE_VERIFIED", "F09", "SYNC", batch);
        spec.respond(
            if verified { "VERIFIED" } else { "NOT_PROVEN" },
            if verified { "NA" } else { "REMOTE_NOT_PROVEN" },
            if verified {
                "Root commits prove the repository's GitHub remote."
            } else {
                "Upload did not prove the repository's remote."
            },
        )
        .set("repository", "repo_remote_verified", verified)
        .set(
            "assessment",
            "assessment",
            if verified { "COMPLIANT" } else { "UNRESOLVED" },
        )
        .set(
            "assessment",
            "assessment_checks",
            ["ROOT_COMMITS_AND_REMOTE"],
        )
        .severity(if verified { "INFO" } else { "MEDIUM" }, "OUTCOME", "NONE");
        record(spec);
    }
    for problem in stored["chain_problems"].as_array().into_iter().flatten() {
        let mut spec = machine_event("EVIDENCE_CHAIN_BROKEN", "F09", "SYNC", batch);
        spec.respond(
            "STORED_WITH_ALERT",
            &problem["kind"]
                .as_str()
                .unwrap_or("gap")
                .to_ascii_uppercase(),
            "Machine's evidence history was rewritten or removed.",
        )
        .set("assessment", "assessment", "SUSPICIOUS")
        .set("assessment", "assessment_checks", ["CHAIN_CONTINUITY"])
        .set("decision", "enforcement", "DETECTED_AFTER_EFFECT")
        .set(
            "decision",
            "decision_reasons",
            [problem["detail"].as_str().unwrap_or_default()],
        )
        .severity("CRITICAL", "FIXED", "EVIDENCE_CHAIN_BROKEN")
        .set("eval", "received_sequence", &problem["sequence"]);
        record(spec);
    }
    if let Some(summary) = &batch.sync {
        if let Some(reason) = summary["trigger_reason"].as_str() {
            // F34.1: only triggers that started an upload reach the service
            let mut spec = machine_event("SYNC_TRIGGER_EVALUATED", "F34", "SYNC_TRIGGER", batch);
            spec.set(
                "action",
                "action_source",
                if reason == "COMMAND" || reason == "INTERVAL" {
                    "CLI"
                } else {
                    "HOOK"
                },
            )
            .set("resource", "resource", ["NOT_CONSUMED"])
            .respond("STARTED", "NA", "Background upload started.")
            .severity("INFO", "FIXED", "NONE")
            .set("bindings", "third_party", ["NONE"])
            .set(
                "eval",
                "interval_seconds",
                summary["interval_seconds"].clone(),
            )
            .set("other", "trigger", reason);
            if summary["seconds_since_last_attempt"].is_number() {
                spec.set(
                    "eval",
                    "seconds_since_last_attempt",
                    summary["seconds_since_last_attempt"].clone(),
                );
            }
            record(spec);
        }
        let mut started = machine_event("SYNC_STARTED", "F09", "SYNC", batch);
        started
            .respond("STARTED", "NA", "Machine started an upload.")
            .severity("INFO", "FIXED", "NONE")
            .set("other", "trigger", summary["trigger"].clone())
            .set("other", "started_at", summary["started_at"].clone());
        record(started);
        if let Some(error) = summary["previous_error"]
            .as_str()
            .filter(|error| *error != "uploads are off")
        {
            let mut refused = machine_event("SYNC_REFUSED", "F09", "SYNC", batch);
            refused
                .respond(
                    "UNREACHABLE",
                    "NETWORK",
                    "Previous upload attempt failed on the machine.",
                )
                .set("decision", "decision_reasons", [error])
                .severity("MEDIUM", "FAILURE_STREAK", "NONE")
                .set("telemetry", "known_as", "INFERRED");
            record(refused);
        }
        if summary["resent"] == true {
            let mut resent = machine_event("SYNC_RESENT", "F09", "SYNC", batch);
            resent
                .respond("OK", "NA", "Records the service lost were sent again.")
                .severity("MEDIUM", "FIXED", "NONE")
                .set("eval", "records_resent", summary["events"].clone());
            record(resent);
        }
        let mut completed = machine_event("SYNC_COMPLETED", "F09", "SYNC", batch);
        completed
            .respond("OK", "NA", "Upload completed.")
            .severity("LOW", "FAILURE_STREAK", "NONE")
            .set("eval", "events_sent", summary["events"].clone())
            .set("eval", "duplicates_skipped", stored["duplicates"].clone())
            .set("other", "status_report_included", batch.status.is_some());
        record(completed);
    }
    if let Some(status) = &batch.status {
        statuses(&record, batch, before.status.as_ref(), status);
    }
}

/** Record a status report and how it differs from the previous one
 * Input
    - record: &dyn Fn(Spec) - records an event
    - batch: &Batch - the batch
    - previous: Option<&Value> - the previous report
    - status: &Value - the new report
 * Output
    - None
*/
fn statuses(record: &dyn Fn(Spec), batch: &Batch, previous: Option<&Value>, status: &Value) {
    let governance = &status["governance"];
    let integrity_ok = status["integrity"]["ok"] == true;
    let findings = status["integrity"]["findings"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let codes = |findings: &[Value]| {
        findings
            .iter()
            .filter_map(|finding| finding["code"].as_str().map(str::to_ascii_uppercase))
            .collect::<Vec<_>>()
    };
    let selections = governance["selections"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let policies = governance["policies"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let passing = policies
        .iter()
        .filter(|policy| policy["passed"] == true)
        .count();
    // F28.1
    let mut reported = machine_event("STATUS_REPORTED", "F28", "STATUS_REPORT", batch);
    let previous_version =
        previous.and_then(|previous| previous["crane_version"].as_str().map(String::from));
    let version = status["crane_version"].as_str().map(String::from);
    reported
        .respond(
            if governance.get("error").is_some() {
                "NOT_INITIALIZED"
            } else if integrity_ok && governance["passed"] != false {
                "RULES_PASS"
            } else {
                "RULES_FAIL"
            },
            if integrity_ok {
                "NA"
            } else {
                "VERIFICATION_FAILED"
            },
            "Machine reported its view of the rules.",
        )
        .set(
            "assessment",
            "assessment",
            if integrity_ok && governance["passed"] != false {
                "COMPLIANT"
            } else {
                "VIOLATION"
            },
        )
        .set(
            "assessment",
            "assessment_checks",
            [
                "INTEGRITY",
                "POLICIES",
                "SELECTIONS",
                "CONTEXT",
                "CONNECTION",
            ],
        )
        .set(
            "agent",
            "autonomy_mode",
            governance["autonomy"]["mode"]
                .as_str()
                .map_or_else(|| "NA".to_string(), str::to_ascii_uppercase),
        )
        .severity(
            if integrity_ok { "INFO" } else { "HIGH" },
            "OUTCOME",
            "NONE",
        )
        .set("eval", "selections", selections.len())
        .set(
            "eval",
            "selections_unresolved",
            selections
                .iter()
                .filter(|selection| selection["resolved"].is_null())
                .count(),
        )
        .set("eval", "policies_passing", passing)
        .set("eval", "policies_total", policies.len())
        .set(
            "eval",
            "checkpoints",
            governance["checkpoints"].as_array().map_or(0, Vec::len),
        )
        .set(
            "eval",
            "admins",
            governance["admins"]["admins"]
                .as_array()
                .map_or(0, Vec::len)
                + 1,
        )
        .set(
            "eval",
            "session_credits",
            governance["autonomy"]["session_credits"].clone(),
        )
        .set("eval", "findings", findings.len())
        .set(
            "other",
            "crane_version_changed",
            previous_version.is_some() && previous_version != version,
        )
        .set("other", "previous_crane_version", previous_version)
        .set(
            "other",
            "default_policy",
            governance["defaults"]["policy"].clone(),
        )
        .set(
            "other",
            "default_checkpoint",
            governance["defaults"]["checkpoint"].clone(),
        )
        .set(
            "other",
            "connection_method",
            status["repo"]["connection"]["method"].as_str().map_or_else(
                || "NONE".to_string(),
                |method| method.replace('-', "_").to_ascii_uppercase(),
            ),
        );
    record(reported);
    // F12: integrity changed
    let was_ok = previous.map(|previous| previous["integrity"]["ok"] == true);
    let blocking = findings
        .iter()
        .filter(|finding| matches!(finding["severity"].as_str(), Some("HIGH" | "CRITICAL")))
        .cloned()
        .collect::<Vec<_>>();
    let warnings = findings
        .iter()
        .filter(|finding| !matches!(finding["severity"].as_str(), Some("HIGH" | "CRITICAL")))
        .cloned()
        .collect::<Vec<_>>();
    let previous_codes = previous
        .map(|previous| {
            codes(
                previous["integrity"]["findings"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
            )
        })
        .unwrap_or_default();
    if !integrity_ok && (was_ok != Some(false) || codes(&blocking) != previous_codes) {
        let critical = blocking
            .iter()
            .any(|finding| finding["severity"] == "CRITICAL");
        let mut spec = machine_event("RULES_INTEGRITY_FAILED", "F12", "VERIFY_RULES", batch);
        spec.respond(
            "FAILED",
            "SECURITY",
            "Machine reports the rules fail verification.",
        )
        .set(
            "resource",
            "resource",
            ["CRANE_METADATA", "TRUST_DIRECTORY"],
        )
        .set("assessment", "assessment", "VIOLATION")
        .set(
            "assessment",
            "assessment_checks",
            [
                "SIGNATURES",
                "SIGNERS",
                "FILE_DIGESTS",
                "CHAINS",
                "ADMIN_EPOCH",
                "ROLLBACK_ANCHOR",
            ],
        )
        .set("decision", "decision", "DENY")
        .set("decision", "decision_rule", "INTEGRITY_FAILED")
        .set(
            "decision",
            "decision_reasons",
            blocking
                .iter()
                .filter_map(|finding| finding["message"].as_str())
                .collect::<Vec<_>>(),
        )
        .set("decision", "enforcement", "PREVENTED")
        .severity(
            if critical { "CRITICAL" } else { "HIGH" },
            "OUTCOME",
            "NONE",
        )
        .set("eval", "blocking_findings", blocking.len())
        .set("eval", "findings", findings.len())
        .set("other", "finding_codes", codes(&blocking));
        record(spec);
    } else if integrity_ok && was_ok == Some(false) {
        let mut spec = machine_event("RULES_INTEGRITY_RESTORED", "F12", "VERIFY_RULES", batch);
        spec.respond("OK", "NA", "Rules verify again on this machine.")
            .set("assessment", "assessment", "COMPLIANT")
            .severity("INFO", "FIXED", "NONE");
        record(spec);
    }
    if integrity_ok && !warnings.is_empty() && codes(&warnings) != previous_codes {
        let mut spec = machine_event("RULES_INTEGRITY_WARNING", "F12", "VERIFY_RULES", batch);
        spec.respond("WARNING", "NA", "Rules verify with warnings.")
            .set("assessment", "assessment", "COMPLIANT")
            .severity("LOW", "OUTCOME", "NONE")
            .set("eval", "findings", warnings.len())
            .set("other", "finding_codes", codes(&warnings));
        record(spec);
    }
    // F18: blocks that moved or broke since the previous report
    let previous_selections = previous
        .and_then(|previous| previous["governance"]["selections"].as_array().cloned())
        .unwrap_or_default();
    for selection in &selections {
        let id = selection["id"].as_str().unwrap_or_default();
        let earlier = previous_selections
            .iter()
            .find(|earlier| earlier["id"] == id);
        let selection_event = |name: &str| {
            let mut spec = machine_event(name, "F18", "TRACK_SELECTION", batch);
            spec.set("resource", "resource", ["SELECTION", "FILE_READ"])
                .set("resource", "resource_ids", [id])
                .set(
                    "resource",
                    "resource_classification",
                    [if selection["operation"] == "target" {
                        "TARGETED"
                    } else {
                        "PROTECTED"
                    }],
                )
                .set("other", "selection_id", id)
                .set(
                    "other",
                    "operation",
                    selection["operation"].as_str().map(str::to_ascii_uppercase),
                )
                .set("other", "policy", selection["policy"].clone());
            spec
        };
        let resolved = &selection["resolved"];
        let was = earlier.map(|earlier| &earlier["resolved"]);
        if resolved.is_null() && was.is_none_or(|was| !was.is_null()) {
            let mut spec = selection_event("SELECTION_UNRESOLVED");
            spec.respond(
                "UNRESOLVED",
                "MARKERS_MISSING",
                "Block can no longer be found by its markers.",
            )
            .set("assessment", "assessment", "UNRESOLVED")
            .set("assessment", "assessment_checks", ["MARKERS"])
            .severity("HIGH", "FIXED", "NONE")
            .set(
                "other",
                "last_known_path",
                was.and_then(|was| was["path"].as_str().map(String::from)),
            );
            record(spec);
        } else if !resolved.is_null() {
            let moved = was.is_some_and(|was| {
                !was.is_null()
                    && (was["path"] != resolved["path"]
                        || was["start_line"] != resolved["start_line"])
            });
            let back = was.is_some_and(Value::is_null);
            if moved || back {
                let mut spec = selection_event("SELECTION_RELOCATED");
                let from = was.and_then(|was| was["path"].as_str().map(String::from));
                spec.respond(
                    "RESOLVED",
                    "NA",
                    if back {
                        "Block found again by its markers."
                    } else {
                        "Block moved; still found by its markers."
                    },
                )
                .set("assessment", "assessment", "COMPLIANT")
                .set("assessment", "assessment_checks", ["MARKERS"])
                .severity("INFO", "FIXED", "NONE")
                .set("other", "from_path", &from)
                .set("other", "to_path", resolved["path"].clone())
                .set(
                    "other",
                    "relocation_kind",
                    if from.as_deref() == resolved["path"].as_str() {
                        "WITHIN_FILE"
                    } else {
                        "RENAMED_FILE"
                    },
                );
                if let (Some(old), Some(new)) = (
                    was.and_then(|was| was["start_line"].as_i64()),
                    resolved["start_line"].as_i64(),
                ) {
                    spec.set("eval", "line_shift", new - old);
                }
                record(spec);
            }
        }
    }
    // Candidates (the original code found elsewhere) and stray markers, when they appear
    let count = |findings: Option<&Value>, test: &dyn Fn(&Value) -> bool| {
        findings.and_then(Value::as_array).map_or(0, |findings| {
            findings.iter().filter(|finding| test(finding)).count()
        })
    };
    let candidate = |finding: &Value| {
        finding["message"]
            .as_str()
            .is_some_and(|message| message.contains("baseline content appears at"))
    };
    let stray = |finding: &Value| finding["code"] == "marker_unregistered";
    for (test, name, statement) in [
        (
            &candidate as &dyn Fn(&Value) -> bool,
            "SELECTION_CANDIDATE_FOUND",
            "Original code of a missing block found elsewhere.",
        ),
        (
            &stray as &dyn Fn(&Value) -> bool,
            "STRAY_MARKERS_FOUND",
            "Markers found that the signed rules do not know.",
        ),
    ] {
        let now = count(governance.get("findings"), test);
        let then = count(
            previous.map(|previous| &previous["governance"]["findings"]),
            test,
        );
        if now > then {
            let mut spec = machine_event(name, "F18", "TRACK_SELECTION", batch);
            spec.respond(
                if name == "STRAY_MARKERS_FOUND" {
                    "FOUND"
                } else {
                    "CANDIDATE"
                },
                if name == "STRAY_MARKERS_FOUND" {
                    "UNREGISTERED_MARKER"
                } else {
                    "NA"
                },
                statement,
            )
            .set(
                "assessment",
                "assessment",
                if name == "STRAY_MARKERS_FOUND" {
                    "SUSPICIOUS"
                } else {
                    "PENDING"
                },
            )
            .severity(
                if name == "STRAY_MARKERS_FOUND" {
                    "HIGH"
                } else {
                    "MEDIUM"
                },
                "FIXED",
                "NONE",
            )
            .set(
                "eval",
                if name == "STRAY_MARKERS_FOUND" {
                    "stray_markers"
                } else {
                    "candidates"
                },
                now,
            );
            record(spec);
        }
    }
    // F20: context files whose state changed
    let previous_contexts = previous
        .and_then(|previous| previous["governance"]["contexts"].as_array().cloned())
        .unwrap_or_default();
    for context in governance["contexts"].as_array().into_iter().flatten() {
        let state = context["state"]
            .as_str()
            .unwrap_or("current")
            .to_ascii_uppercase();
        let earlier = previous_contexts
            .iter()
            .find(|earlier| {
                earlier["path"] == context["path"] && earlier["policy"] == context["policy"]
            })
            .and_then(|earlier| earlier["state"].as_str().map(str::to_ascii_uppercase));
        if earlier.as_deref().is_some_and(|earlier| earlier != state)
            || (earlier.is_none() && state != "CURRENT")
        {
            let mut spec = machine_event("CONTEXT_STATE_CHANGED", "F20", "CONTEXT", batch);
            spec.respond(
                &state,
                match state.as_str() {
                    "STALE" => "FILE_CHANGED",
                    "MISSING" => "FILE_MISSING",
                    _ => "NA",
                },
                "Policy context file changed state.",
            )
            .set(
                "assessment",
                "assessment",
                if state == "CURRENT" {
                    "COMPLIANT"
                } else {
                    "UNRESOLVED"
                },
            )
            .set("assessment", "assessment_checks", ["CONTEXT_DIGEST"])
            .severity(
                if state == "CURRENT" { "INFO" } else { "MEDIUM" },
                "OUTCOME",
                "NONE",
            )
            .set("resource", "resource", ["FILE_READ", "CRANE_METADATA"])
            .set("other", "policy", context["policy"].clone())
            .set("other", "path", context["path"].clone());
            record(spec);
        }
    }
}
