// The tool-call phases of the hook runtime: deciding a tool call before it runs (the guard,
// autonomy credits, zones and flows), and verifying its actual effects after it ran (changes to
// protected code, markers, and governance files, and shell commands that bypassed a check).

use super::*;
use crate::governance::state::integrity;
use crate::governance::verify::{evaluate, Options, ScanMode};
use crate::hooks::action::Operation;
use crate::hooks::adapter::{Feedback, HookEvent, Verdict};
use crate::hooks::enforce::{repository_path, Decided, Guard};
use crate::platform::now_millis;
use crate::selection::registry::{Record, Status};
use crate::telemetry::events::Measurement;
use crate::telemetry::identity::SessionRecord;
use crate::telemetry::ledger::Kind;
use crate::telemetry::model::{
    Assessment, Decision, Domain, Enforcement, Execution, Safety, Severity, Telemetry,
};
use crate::trust::crypto::sha512_hex;
use serde_json::json;
use std::path::PathBuf;

impl Hook {
    /** PreToolUse and PermissionRequest: decide the tool call, reserve its credits, count bypass
     * attempts (quarantining the session past the threshold), record the decision, and answer
     * Input
        - session: &mut SessionRecord - session
     * Output
        - Result<(), String>
    */
    pub(super) fn pre_tool(&self, session: &mut SessionRecord) -> Result<(), String> {
        let action = self
            .provider
            .action
            .clone()
            .ok_or("the hook payload has no tool call")?;
        let (call, provided) = self.tool_call_id(session, &action);
        let trust = self.workspace.trust()?;
        let integrity_ok = !integrity(&self.workspace, &self.state, &trust)
            .iter()
            .any(crate::governance::Finding::blocking);
        let available = self.available(session)?;
        let (limits, zone_error) = match crate::governance::zones::cached_limits(
            &self.workspace,
            &self.state,
            &self.stores.directory,
        ) {
            Ok(limits) => (limits, None),
            Err(error) => (Default::default(), Some(error)),
        };
        let guard = Guard {
            workspace: &self.workspace,
            state: &self.state,
            integrity_ok,
            protected_paths: self.adapter.protected_paths(),
            trust_home: self.trust_home.clone(),
            cwd: self.provider.cwd.clone().map(PathBuf::from),
            mode: session.autonomy_mode.clone(),
            safety: session.safety,
            available: Some(available),
            costs: self.state.config.autonomy.costs.clone(),
            limits: Some(&limits),
            zone_error,
        };
        let mut decided: Decided = guard.decide(&action);
        // Guidance of the active zones and flows covering the files this call reads or writes
        let mut path_candidates = action
            .files
            .iter()
            .map(|change| change.path.as_str())
            .chain(action.reads.iter().map(String::as_str))
            .collect::<Vec<_>>();
        let command_paths = action
            .command
            .as_deref()
            .map(crate::hooks::shell::words)
            .unwrap_or_default();
        path_candidates.extend(command_paths.iter().filter_map(|candidate| {
            let relative = guard.relative(candidate)?;
            self.workspace
                .root
                .join(&relative)
                .is_file()
                .then_some(candidate.as_str())
        }));
        let touched = path_candidates
            .into_iter()
            .filter_map(|raw| guard.relative(raw))
            .collect::<std::collections::BTreeSet<_>>();
        let entities = touched
            .iter()
            .filter_map(|file| limits.get(file))
            .flat_map(|limit| limit.entities.iter().cloned())
            .collect::<std::collections::BTreeSet<_>>();
        let governance = if entities.is_empty() {
            None
        } else {
            Some(crate::governance::zones::load(&self.workspace)?)
        };
        let zone_guidance = governance
            .as_ref()
            .map(|governance| {
                crate::governance::zones::guidance(&self.workspace, governance, &entities)
            })
            .filter(|text| !text.is_empty());
        let mut selection_guidance = Vec::new();
        let mut context_candidates = Vec::new();
        for record in self.state.registry.selections.iter().filter(|record| {
            record.status == Status::Active
                && (touched.contains(&record.current.span.path)
                    || decided.selections.contains(&record.id))
        }) {
            let text = self.selection_guidance(record)?;
            let entity = format!("selection:{}", record.id);
            context_candidates.push((entity, text.clone()));
            selection_guidance.push(text);
        }
        let guidance = [
            zone_guidance,
            (!selection_guidance.is_empty()).then(|| {
                format!(
                    "Crane selection guidance (guidance only, never authorization):\n{}",
                    selection_guidance.join("\n")
                )
            }),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        let guidance = (!guidance.is_empty()).then(|| guidance.join("\n"));
        if let Some(governance) = &governance {
            for entity in &entities {
                let single = std::collections::BTreeSet::from([entity.clone()]);
                let text = crate::governance::zones::guidance(&self.workspace, governance, &single);
                if !text.is_empty() {
                    context_candidates.push((entity.clone(), text));
                }
            }
        }
        let mut reserved = 0;
        if decided.verdict.decision == Decision::Allow && decided.verdict.cost > 0 {
            reserved = self.ledger(
                session,
                Some(&call),
                Kind::Reserve,
                decided.verdict.cost,
                &format!("{} {}", action.operation.name(), action.tool),
                &format!("{call}:reserve"),
            )?;
            if reserved < decided.verdict.cost {
                self.ledger(
                    session,
                    Some(&call),
                    Kind::Release,
                    reserved,
                    "insufficient credits",
                    &format!("{call}:release-short"),
                )?;
                reserved = 0;
                decided.verdict.decision = Decision::RequireApproval;
                decided.verdict.reasons.push(
                    "autonomy credits were exhausted by concurrent actions; a human must approve"
                        .into(),
                );
                decided.rules.push("CREDITS_SHORT");
            }
        }
        let threshold = self.state.config.autonomy.quarantine_after_bypass_attempts;
        let mut quarantined_now = false;
        if decided.verdict.bypass {
            self.update_session(session, |session| {
                session.bypass_attempts += 1;
                if session.bypass_attempts >= threshold && session.safety != Safety::Quarantined {
                    session.safety = Safety::Quarantined;
                    session.safety_reason = Some(format!(
                        "{} attempts to bypass Crane",
                        session.bypass_attempts
                    ));
                    quarantined_now = true;
                }
            })?;
        }
        // What this call may change, so the stop hook can tell its changes from changes made
        // outside every recorded tool call: the files a write names, and for a shell or other
        // tool (whose effect is unknown beforehand) the changed files before it runs
        let allowed = decided.verdict.decision != Decision::Deny
            && decided.verdict.decision != Decision::Quarantine;
        let written = action
            .files
            .iter()
            .filter_map(|change| {
                repository_path(
                    &self.workspace,
                    self.provider.cwd.as_deref().map(std::path::Path::new),
                    &change.path,
                )
            })
            .collect::<Vec<_>>();
        let before = (allowed && matches!(action.operation, Operation::Execute | Operation::Other))
            .then(|| tree(&self.workspace).ok())
            .flatten();
        let baseline = self
            .with_calls(session, |calls| calls.baseline.is_none())?
            .then(|| tree(&self.workspace).ok())
            .flatten();
        self.with_calls(session, |calls| {
            if calls.baseline.is_none() {
                calls.baseline = baseline;
            }
            if allowed {
                calls.attributed.extend(written);
            }
            calls.calls.insert(
                call.clone(),
                CallRecord {
                    at: now_millis(),
                    decision: decided.verdict.decision,
                    cost: decided.verdict.cost,
                    reserved,
                    operation: action.operation.name().into(),
                    tool: action.tool.clone(),
                    selections: decided.selections.clone(),
                    before,
                },
            );
        })?;
        let domain = if decided.verdict.bypass {
            Domain::PolicyIntegrity
        } else if !decided.network.is_empty() {
            Domain::NetworkExfiltration
        } else {
            Domain::Authorization
        };
        let mut event = self.base_event(
            "tool_call.decided",
            domain,
            session,
            &format!("{call}:{}", self.event.name()),
        );
        if !provided {
            event
                .idempotency_key
                .push_str(&format!(":{}", now_millis()));
        }
        event.scope.tool_call_id = Some(call.clone());
        event.scope.span_id = Some(call.clone());
        event.decision = Some(decided.verdict.decision);
        event.execution =
            (decided.verdict.decision != Decision::Allow).then_some(Execution::NotExecuted);
        event.assessment =
            (decided.verdict.decision != Decision::Allow).then_some(if decided.verdict.bypass {
                Assessment::Suspicious
            } else {
                Assessment::Violation
            });
        event.enforcement = if matches!(
            decided.verdict.decision,
            Decision::Deny | Decision::Quarantine
        ) {
            Enforcement::Prevented
        } else {
            Enforcement::NotApplicable
        };
        event.severity = match decided.verdict.decision {
            Decision::Allow => Severity::Info,
            Decision::RequireApproval => Severity::Low,
            _ if decided.verdict.bypass => Severity::High,
            _ => Severity::Medium,
        };
        event.telemetry = if provided {
            Telemetry::Observed
        } else {
            Telemetry::Inferred
        };
        event.bindings.policies = decided.policies.clone();
        event.bindings.selections = decided.selections.clone();
        event.bindings.zones = decided.zones.clone();
        event.resources = decided.resources.clone();
        event.reasons = decided.verdict.reasons.clone();
        event.measurements.push(Measurement::observed(
            "credits_cost",
            decided.verdict.cost as f64,
            "credits",
            "cost model",
        ));
        event.measurements.push(Measurement::observed(
            "credits_reserved",
            reserved as f64,
            "credits",
            "autonomy ledger",
        ));
        event.measurements.push(Measurement::observed(
            "credits_available_before",
            available as f64,
            "credits",
            "autonomy ledger",
        ));
        event.measurements.push(Measurement::observed(
            "bypass_attempt",
            if decided.verdict.bypass { 1.0 } else { 0.0 },
            "count",
            "pre-action decision",
        ));
        if !decided.network.is_empty() {
            event.measurements.push(Measurement {
                name: "network_destinations".into(),
                value: Some(decided.network.len() as f64),
                unit: "count".into(),
                status: Telemetry::Inferred,
                method: "parsed from the command or tool arguments; not observed on the network"
                    .into(),
            });
        }
        event.measurements.push(Measurement::missing(
            "network_bytes_sent",
            "bytes",
            Telemetry::Unsupported,
            "Crane does not observe the network stack",
        ));
        event.payload["tool"] = json!(action.tool);
        event.payload["operation"] = json!(action.operation.name());
        event.payload["input_digest"] = json!(action.digest);
        event.payload["network"] = json!(decided.network);
        event.payload["tool_call_id_provenance"] =
            json!(if provided { "provider" } else { "generated" });
        event.payload["rules"] = json!(decided.rules);
        if !decided.scripts.is_empty() {
            event.payload["scripts"] = json!(decided.scripts);
        }
        if let Some(text) = &guidance {
            if decided.verdict.decision != Decision::Deny
                && decided.verdict.decision != Decision::Quarantine
            {
                event.payload["guidance_bytes"] = json!(text.len());
            }
        }
        // The strictest zone or flow limits among the files this call touches
        let touched_limits = touched
            .iter()
            .filter_map(|file| limits.get(file))
            .collect::<Vec<_>>();
        if let Some(criticality) = touched_limits.iter().map(|limit| limit.criticality).max() {
            event.payload["criticality"] = json!(criticality);
        }
        if let Some(ceiling) = touched_limits.iter().map(|limit| limit.ceiling).min() {
            event.payload["ceiling"] = json!(ceiling);
        }
        event.payload["credits_available"] = json!(available);
        let alert = if decided.verdict.bypass {
            Some("bypass_attempt")
        } else {
            None
        };
        self.emit(session, event, alert)?;
        if quarantined_now {
            let mut event = self.base_event(
                "session.quarantined",
                Domain::RecoveryOversight,
                session,
                "quarantined",
            );
            event.severity = Severity::Critical;
            event.decision = Some(Decision::Quarantine);
            event
                .reasons
                .push(session.safety_reason.clone().unwrap_or_default());
            self.emit(session, event, Some("quarantine"))?;
        }
        if decided.verdict.decision == Decision::Allow {
            let remaining = available.saturating_sub(reserved);
            if remaining <= self.state.config.autonomy.low_credit_threshold
                && available > self.state.config.autonomy.low_credit_threshold
            {
                let mut event = self.base_event(
                    "credits.low",
                    Domain::ResourceAbuse,
                    session,
                    &format!("{call}:low"),
                );
                event.severity = Severity::Medium;
                event.measurements.push(Measurement::observed(
                    "credits_available",
                    remaining as f64,
                    "credits",
                    "autonomy ledger",
                ));
                self.emit(session, event, Some("low_credits"))?;
            }
        }
        let notices = context_candidates
            .into_iter()
            .map(|(entity, text)| format!("Crane context · {entity}:\n{text}"))
            .collect::<Vec<_>>();
        let system_message = (!notices.is_empty()).then(|| notices.join("\n"));
        let verdict = Verdict {
            context: guidance,
            system_message,
            ..decided.verdict
        };
        if self.event == HookEvent::PermissionRequest {
            self.adapter.respond_permission(&verdict)
        } else {
            self.adapter.respond_action(&verdict)
        }
    }

    fn selection_guidance(&self, record: &Record) -> Result<String, String> {
        let requirement = match record.operation {
            crate::selection::registry::Operation::Preserve => "must remain unchanged".to_string(),
            crate::selection::registry::Operation::Target => record
                .change_type
                .as_deref()
                .map(|change| format!("must be changed meaningfully ({change})"))
                .unwrap_or_else(|| "must be changed meaningfully".into()),
        };
        let mut text = format!(
            "Selection {}{} ({}) in policy {} at {} lines {}-{}.",
            record.id,
            record
                .name
                .as_deref()
                .map(|name| format!(" '{name}'"))
                .unwrap_or_default(),
            requirement,
            record.policy,
            record.current.span.path,
            record.current.span.start_line,
            record.current.span.end_line,
        );
        for binding in self.state.context.of(&record.policy) {
            let path = self.workspace.root.join(&binding.path);
            let bytes = fs::read(&path).map_err(|error| {
                format!("could not read selection context {}: {error}", binding.path)
            })?;
            let current_digest = sha512_hex(&bytes);
            text.push_str(&format!(
                "\nAttached policy context ({}{}):\n{}",
                binding.path,
                if current_digest == binding.digest {
                    ""
                } else {
                    ", changed since it was bound"
                },
                String::from_utf8_lossy(&bytes).trim_end()
            ));
        }
        const LIMIT: usize = 8_000;
        if text.len() > LIMIT {
            let mut end = LIMIT;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
            text.push_str("\n[selection context truncated]");
        }
        Ok(text)
    }

    /** PostToolUse: settle the reservation, verify the actual effects (written files, or the
     * whole work tree after a shell or other tool), attribute new violations to this tool call
     * (a protected change that did not pass a write check is a confirmed bypass), degrade or
     * quarantine the session, and tell the agent how to repair
     * Input
        - session: &mut SessionRecord - session
     * Output
        - Result<(), String>
    */
    pub(super) fn post_tool(&self, session: &mut SessionRecord) -> Result<(), String> {
        let action = self
            .provider
            .action
            .clone()
            .ok_or("the hook payload has no tool call")?;
        let (call, provided) = self.tool_call_id(session, &action);
        let record = self.with_calls(session, |calls| calls.calls.get(&call).cloned())?;
        let failed = self.provider.tool_failed;
        let mut attributed_files = None;
        if let Some(before) = record.as_ref().and_then(|record| record.before.as_ref()) {
            if let Ok(after) = tree(&self.workspace) {
                let changed = tree_changes(before, &after);
                attributed_files = Some(changed.len());
                self.with_calls(session, |calls| calls.attributed.extend(changed))?;
            }
        }
        if let Some(record) = &record {
            if record.reserved > 0 {
                if failed == Some(true) && action.operation == Operation::Write {
                    self.ledger(
                        session,
                        Some(&call),
                        Kind::Release,
                        record.reserved,
                        "the write failed",
                        &format!("{call}:release"),
                    )?;
                } else {
                    self.ledger(
                        session,
                        Some(&call),
                        Kind::Consume,
                        record.reserved,
                        "the action ran",
                        &format!("{call}:consume"),
                    )?;
                }
            }
        } else {
            let mut event = self.base_event(
                "hook.missing_decision",
                Domain::AuditCompliance,
                session,
                &format!("{call}:missing"),
            );
            event.scope.tool_call_id = Some(call.clone());
            event.severity = Severity::High;
            event.enforcement = Enforcement::CoverageGap;
            event.telemetry = Telemetry::Unobserved;
            event.reasons.push(format!("{} ran without a recorded pre-tool authorization (hook missing, failed, or the tool bypassed it)", action.tool));
            self.emit(session, event, Some("coverage_gap"))?;
        }
        let scan = match action.operation {
            Operation::Write => ScanMode::Paths(
                action
                    .files
                    .iter()
                    .filter_map(|change| {
                        repository_path(
                            &self.workspace,
                            self.provider.cwd.as_deref().map(std::path::Path::new),
                            &change.path,
                        )
                    })
                    .collect(),
            ),
            Operation::Read => ScanMode::Paths(Vec::new()),
            _ => ScanMode::Full,
        };
        let (report, state) = evaluate(
            &self.workspace,
            &Options {
                scan,
                targets_pending: true,
                candidates: false,
                governance: false,
            },
        )?;
        let violations = violations(&report);
        let new = self.with_calls(session, |calls| {
            let new = violations
                .keys()
                .filter(|key| !calls.known_violations.contains(*key))
                .cloned()
                .collect::<Vec<_>>();
            calls.known_violations = violations.keys().cloned().collect();
            new
        })?;
        let mut event = self.base_event(
            "tool_call.verified",
            Domain::PolicyIntegrity,
            session,
            &format!("{call}:post"),
        );
        if !provided {
            event
                .idempotency_key
                .push_str(&format!(":{}", now_millis()));
        }
        event.scope.tool_call_id = Some(call.clone());
        event.scope.span_id = Some(call.clone());
        event.decision = record.as_ref().map(|record| record.decision);
        event.execution = Some(match failed {
            Some(true) => Execution::Failure,
            Some(false) => Execution::Success,
            None => Execution::Unknown,
        });
        if let Some(record) = &record {
            event.measurements.push(Measurement::observed(
                "tool_duration_ms",
                now_millis().saturating_sub(record.at) as f64,
                "ms",
                "pre-tool to post-tool hook interval",
            ));
        } else {
            event.measurements.push(Measurement::missing(
                "tool_duration_ms",
                "ms",
                Telemetry::Unobserved,
                "no pre-tool decision was recorded",
            ));
        }
        // The model turn that issued the call, from the agent's transcript (DASHBOARD.md F06)
        let usage = self
            .provider
            .transcript
            .as_deref()
            .zip(self.provider.tool_call_id.as_deref())
            .and_then(|(path, id)| {
                crate::telemetry::usage::of_tool_call(std::path::Path::new(path), id)
            });
        match &usage {
            Some(turn) => {
                let share = turn.share();
                for (name, value) in [
                    ("model_tokens", share.total()),
                    ("model_input_tokens", share.input),
                    ("model_output_tokens", share.output),
                    ("cache_read_tokens", share.cache_read),
                    ("cache_creation_tokens", share.cache_creation),
                    ("turn_tokens", turn.total()),
                    ("turn_tool_calls", turn.calls as f64),
                ] {
                    event.measurements.push(Measurement::observed(
                        name,
                        (value * 10.0).round() / 10.0,
                        if name == "turn_tool_calls" { "count" } else { "tokens" },
                        "the issuing model turn's usage in the agent transcript, shared equally by its tool calls",
                    ));
                }
                if let Some(model) = &turn.model {
                    event.payload["model_turn"] = json!(model);
                }
            }
            None => event.measurements.push(Measurement::missing(
                "model_tokens",
                "tokens",
                Telemetry::Unsupported,
                "the agent reported no transcript with token usage for this call",
            )),
        }
        event.measurements.push(Measurement::observed(
            "selections_verified",
            report.resolutions.len() as f64,
            "count",
            "post-action reconciliation",
        ));
        event.measurements.push(Measurement::observed(
            "files_scanned",
            report.files_scanned as f64,
            "count",
            "post-action reconciliation",
        ));
        event.measurements.push(Measurement::missing(
            "cpu_ms",
            "ms",
            Telemetry::Unsupported,
            "process resource usage is not instrumented",
        ));
        event.measurements.push(Measurement::missing(
            "memory_bytes",
            "bytes",
            Telemetry::Unsupported,
            "process resource usage is not instrumented",
        ));
        event.payload["tool"] = json!(action.tool);
        event.payload["operation"] = json!(action.operation.name());
        event.payload["tool_failed"] = json!(failed);
        event.payload["scan_mode"] = json!(match action.operation {
            Operation::Write => "PATHS",
            Operation::Read => "NONE",
            _ => "FULL",
        });
        if let Some(files) = attributed_files {
            event.payload["attributed_files"] = json!(files);
        }
        if new.is_empty() {
            event.assessment = Some(if violations.is_empty() {
                Assessment::Compliant
            } else {
                Assessment::Unresolved
            });
            if !violations.is_empty() {
                event
                    .reasons
                    .push("violations that existed before this tool call remain".into());
            }
            self.emit(session, event, None)?;
            let summary = if violations.is_empty() {
                String::new()
            } else {
                format!(
                    "{} pre-existing Crane violation(s) remain; a human must repair them",
                    violations.len()
                )
            };
            return self.adapter.respond_feedback(
                HookEvent::PostToolUse,
                &Feedback {
                    block: false,
                    summary,
                    detail: String::new(),
                },
            );
        }
        let bypass = action.operation != Operation::Write;
        event.assessment = Some(Assessment::Violation);
        event.enforcement = if bypass {
            Enforcement::BypassConfirmed
        } else {
            Enforcement::DetectedAfterEffect
        };
        event.severity = if bypass {
            Severity::Critical
        } else {
            Severity::High
        };
        event.reasons = new.iter().map(|key| violations[key].clone()).collect();
        event.bindings.selections = new
            .iter()
            .filter_map(|key| key.strip_prefix("selection:").map(String::from))
            .collect();
        let penalty = self.state.config.autonomy.costs.violation_penalty;
        let revoked = self.ledger(
            session,
            Some(&call),
            Kind::Revoke,
            penalty,
            "violation detected after the effect",
            &format!("{call}:revoke"),
        )?;
        event.measurements.push(Measurement::observed(
            "credits_revoked",
            revoked as f64,
            "credits",
            "autonomy ledger",
        ));
        self.update_session(session, |session| {
            session.violations += 1;
            let target = if bypass {
                Safety::Quarantined
            } else {
                Safety::Degraded
            };
            if target > session.safety {
                session.safety = target;
                session.safety_reason = Some(event.reasons.join("; "));
            }
        })?;
        self.emit(
            session,
            event,
            Some(if bypass {
                "bypass_confirmed"
            } else {
                "violation"
            }),
        )?;
        let detail = repair_instructions(&self.workspace, &state, &report, &new, &violations);
        self.adapter.respond_feedback(
            HookEvent::PostToolUse,
            &Feedback {
                block: true,
                summary: format!(
                    "Crane detected {} violation(s) caused by this {} call{}; restore the protected code",
                    new.len(),
                    action.tool,
                    if bypass { " (protected code changed outside the edit tools: confirmed bypass, session quarantined)" } else { "" }
                ),
                detail,
            },
        )
    }
}
