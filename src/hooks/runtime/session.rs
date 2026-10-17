// The session lifecycle of the hook runtime: starting a session (identity, attestation of the
// starting state, and the policy context supplied to the agent), recording prompts, reconciling
// every change when the agent stops, and attesting the end of the session.

use super::*;
use crate::governance::policy::PolicySet;
use crate::governance::verify::{evaluate, Options, Outcome};
use crate::hooks::adapter::{Feedback, HookEvent};
use crate::platform::now_millis;
use crate::selection::registry::Operation as SelectionOperation;
use crate::telemetry::events::Measurement;
use crate::telemetry::identity::{SessionRecord, SuppliedContext};
use crate::telemetry::ledger::Kind;
use crate::telemetry::model::{Assessment, Domain, Enforcement, Severity, Telemetry};
use crate::trust::crypto::{digest, sha512_hex};
use serde_json::json;
use std::fs;

impl Hook {
    /** SessionStart: grant credits to a new session, supply policies, selections, and policy
     * context, and record the start or resumption
     * Input
        - session: &mut SessionRecord - session
     * Output
        - Result<(), String>
    */
    pub(super) fn session_start(&self, session: &mut SessionRecord) -> Result<(), String> {
        let autonomy = &self.state.config.autonomy;
        let granted = self.ledger(
            session,
            None,
            Kind::Grant,
            autonomy.session_credits,
            "session start grant",
            "grant",
        )?;
        let (context, supplied) = self.context_text(session)?;
        // The files already changed when the session starts are not the agent's doing
        let mut baseline_files = None;
        if self.with_calls(session, |calls| calls.baseline.is_none())? {
            if let Ok(baseline) = tree(&self.workspace) {
                baseline_files = Some(baseline.len());
                self.with_calls(session, |calls| {
                    calls.baseline.get_or_insert(baseline);
                })?;
            }
        }
        // Whether the rules verify: when they do not, every change of this session is denied
        let blocking = self
            .workspace
            .trust()
            .map(|trust| {
                crate::governance::state::integrity(&self.workspace, &self.state, &trust)
                    .into_iter()
                    .filter(crate::governance::Finding::blocking)
                    .map(|finding| finding.code.to_ascii_uppercase())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let policies = PolicySet::load(&self.workspace)
            .map(|policies| policies.blocks().len())
            .unwrap_or(0);
        self.update_session(session, |session| {
            session.context = supplied.clone();
            if session.ended_at.is_some() {
                session.ended_at = None;
                session.end_reason = None;
            }
        })?;
        let mut event = self.base_event(
            "session.started",
            Domain::IdentityProvenance,
            session,
            &format!("start:{}", self.provider.source.clone().unwrap_or_default()),
        );
        event.measurements.push(Measurement::observed(
            "credits_granted",
            granted as f64,
            "credits",
            "autonomy ledger",
        ));
        event.measurements.push(Measurement::observed(
            "context_files_supplied",
            supplied.len() as f64,
            "count",
            "policy context bindings",
        ));
        event.telemetry = if session.provider_session_id.is_some() {
            Telemetry::Observed
        } else {
            Telemetry::Inferred
        };
        event.payload["source"] = json!(self.provider.source);
        event.payload["context"] = json!(supplied);
        event.payload["policies"] = json!(policies);
        event.payload["selections"] = json!(self.state.registry.selections.len());
        event.payload["integrity_ok"] = json!(blocking.is_empty());
        if !blocking.is_empty() {
            event.payload["finding_codes"] = json!(blocking);
        }
        if let Some(files) = baseline_files {
            event.payload["baseline_files"] = json!(files);
        }
        event.evidence = supplied
            .iter()
            .map(|context| format!("context:{}:{}", context.path, &context.digest[..16]))
            .collect();
        self.emit(session, event, None)?;
        self.adapter.respond_context(&context)
    }

    /** Build the session context: the rules of the boundary, every policy with its commands and
     * selections, the policy context files, and the session's autonomy
     * Input
        - session: &SessionRecord - session
     * Output
        - Result<(String, Vec<SuppliedContext>), String> text and the context files supplied
    */
    pub(super) fn context_text(
        &self,
        session: &SessionRecord,
    ) -> Result<(String, Vec<SuppliedContext>), String> {
        let policies = PolicySet::load(&self.workspace)?;
        let mut text = String::from(
            "Crane governs this repository. Agent changes to .crane (including .crane/map), Crane's hook settings, and Crane's trust directory are denied, even with approval; Crane commands are for humans only: do not run crane (any command, including validate and test); Crane checks your work itself. Do not add, move, or alter '@crane:selection' marker comments.\n",
        );
        let mut supplied = Vec::new();
        for (_, block) in policies.blocks() {
            if block.statements.is_empty() {
                continue;
            }
            text.push_str(&format!("Policy {}:\n", block.name));
            for statement in &block.statements {
                let location = self
                    .state
                    .record(&statement.id)
                    .map(|record| {
                        format!(
                            "{} near lines {}-{}",
                            record.current.span.path,
                            record.current.span.start_line,
                            record.current.span.end_line
                        )
                    })
                    .unwrap_or_else(|| "unregistered".into());
                let meaning = match statement.operation {
                    SelectionOperation::Preserve => "must stay exactly as it is".to_string(),
                    SelectionOperation::Target => format!(
                        "must be changed{}",
                        statement
                            .change_type
                            .map(|change_type| format!(" ({})", change_type.name()))
                            .unwrap_or_default()
                    ),
                };
                text.push_str(&format!(
                    "  - {} {} ({location}): {meaning}\n",
                    statement.operation.name(),
                    statement.id
                ));
            }
            for binding in self.state.context.of(&block.name) {
                if let Ok(bytes) = fs::read(self.workspace.root.join(&binding.path)) {
                    let current = sha512_hex(&bytes);
                    text.push_str(&format!(
                        "  Context ({}{}), guidance only:\n{}\n",
                        binding.path,
                        if current == binding.digest {
                            ""
                        } else {
                            ", changed since it was bound"
                        },
                        String::from_utf8_lossy(&bytes).trim_end()
                    ));
                    supplied.push(SuppliedContext {
                        policy: block.name.clone(),
                        path: binding.path.clone(),
                        digest: current,
                    });
                }
            }
        }
        if let Ok(governance) = crate::governance::zones::load(&self.workspace) {
            use crate::governance::zones::{compose_context, Lifecycle};
            let packs = compose_context(&self.workspace, &governance);
            if !packs.is_empty() {
                text.push_str("Context packs (guidance only, never authorization):\n");
            }
            for (id, version, body) in packs {
                text.push_str(&format!("  [{id} v{version}]\n{body}\n"));
                supplied.push(SuppliedContext {
                    policy: "context-pack".into(),
                    path: format!("pack:{id}@v{version}"),
                    digest: sha512_hex(body.as_bytes()),
                });
            }
            for zone in governance
                .zones
                .iter()
                .filter(|zone| zone.status == Lifecycle::Active)
            {
                text.push_str(&format!(
                    "Zone {} ({:?}, autonomy ceiling {}): {}\n",
                    zone.id,
                    zone.criticality,
                    zone.autonomy_ceiling.name(),
                    zone.description
                ));
            }
            for flow in governance
                .flows
                .iter()
                .filter(|flow| flow.status == Lifecycle::Active)
            {
                text.push_str(&format!(
                    "Flow {} ({:?}, autonomy ceiling {}): {}\n",
                    flow.id,
                    flow.criticality,
                    flow.autonomy_ceiling.name(),
                    flow.description
                ));
            }
        }
        let available = self.available(session)?;
        text.push_str(&format!(
            "Autonomy: mode {}, safety {:?}, {available} credits available. Credits measure risk-bearing authority; when they run out, changes need human approval.\n",
            session.autonomy_mode, session.safety
        ));
        Ok((text, supplied))
    }

    /** UserPromptSubmit: open a new task and flag prompts that look like instruction attacks
     * Input
        - session: &mut SessionRecord - session
     * Output
        - Result<(), String>
    */
    pub(super) fn prompt(&self, session: &mut SessionRecord) -> Result<(), String> {
        let prompt = self.provider.prompt.clone();
        let task = {
            let _lock = self.stores.identity.lock()?;
            let mut fresh = self
                .stores
                .identity
                .session(&session.foxx_session_id)
                .unwrap_or_else(|| session.clone());
            let task = self.stores.identity.start_task(
                &mut fresh,
                self.provider.task.clone(),
                Some(prompt.as_deref().unwrap_or_default()),
                now_millis(),
            )?;
            self.stores.identity.save_session(&fresh)?;
            *session = fresh;
            task
        };
        let mut event = self.base_event(
            "task.started",
            Domain::IdentityProvenance,
            session,
            &task.foxx_task_id,
        );
        event.payload["prompt_chars"] = json!(task.prompt_chars);
        event.payload["prompt_digest"] = json!(task.prompt_digest);
        event.payload["task_provenance"] = json!(task.provenance);
        event.evidence.push(format!(
            "prompt-sha512:{}",
            task.prompt_digest.clone().unwrap_or_default()
        ));
        self.emit(session, event, None)?;
        let lowered = prompt.unwrap_or_default().to_ascii_lowercase();
        let matched = INJECTION_PHRASES
            .iter()
            .filter(|phrase| lowered.contains(*phrase))
            .copied()
            .collect::<Vec<_>>();
        if !matched.is_empty() {
            let mut event = self.base_event(
                "prompt.suspicious",
                Domain::PromptInjection,
                session,
                &format!("{}:injection", task.foxx_task_id),
            );
            event.severity = Severity::Medium;
            event.assessment = Some(Assessment::Suspicious);
            event.telemetry = Telemetry::Inferred;
            event.reasons.push(format!(
                "the prompt contains instruction-integrity phrases: {}",
                matched.join(", ")
            ));
            self.emit(session, event, Some("prompt_injection"))?;
        }
        Ok(())
    }

    /** Stop: reconcile completely; a failing policy or unresolved selection blocks the first stop
     * so the agent repairs or completes its targets, and is let through (recorded unverified)
     * the second time so the agent cannot loop
     * Input
        - session: &mut SessionRecord - session
     * Output
        - Result<(), String>
    */
    pub(super) fn stop(&self, session: &mut SessionRecord) -> Result<(), String> {
        self.report_unattributed(session)?;
        let (report, state) = evaluate(
            &self.workspace,
            &Options {
                governance: false,
                ..Options::default()
            },
        )?;
        let failures = report.failures();
        let blocking = report
            .findings
            .iter()
            .filter(|finding| finding.blocking())
            .collect::<Vec<_>>();
        let task = self.task(session);
        let passed = failures.is_empty() && blocking.is_empty();
        let status = if passed {
            "COMPLETED"
        } else if self.provider.stop_hook_active {
            "ENDED"
        } else {
            "BLOCKED"
        };
        if let Some(mut task) = task.clone() {
            task.status = status.into();
            if status != "BLOCKED" {
                task.ended_at = Some(now_millis());
            }
            self.stores.identity.save_task(&task)?;
        }
        let mut event = self.base_event(
            if passed {
                "task.verified"
            } else {
                "task.verification_failed"
            },
            Domain::PolicyIntegrity,
            session,
            &format!(
                "{}:stop:{}",
                task.as_ref()
                    .map(|task| task.foxx_task_id.clone())
                    .unwrap_or_default(),
                now_millis()
            ),
        );
        event.assessment = Some(if passed {
            Assessment::Compliant
        } else if failures
            .iter()
            .any(|failure| failure.outcome == Outcome::Unresolved)
        {
            Assessment::Unresolved
        } else {
            Assessment::Violation
        });
        event.severity = if passed {
            Severity::Info
        } else {
            Severity::High
        };
        event.reasons = failures
            .iter()
            .map(|failure| format!("{}: {}", failure.policy, failure.message))
            .chain(blocking.iter().map(|finding| finding.message.clone()))
            .collect();
        event.measurements.push(Measurement::observed(
            "policies_passed",
            report
                .policies
                .iter()
                .filter(|policy| policy.passed)
                .count() as f64,
            "count",
            "full reconciliation",
        ));
        event.measurements.push(Measurement::observed(
            "policies_failed",
            report
                .policies
                .iter()
                .filter(|policy| !policy.passed)
                .count() as f64,
            "count",
            "full reconciliation",
        ));
        event.payload["task_status"] = json!(status);
        self.emit(session, event, (!passed).then_some("verification_failed"))?;
        if passed {
            return self.adapter.respond_feedback(
                HookEvent::Stop,
                &Feedback {
                    block: false,
                    summary: String::new(),
                    detail: String::new(),
                },
            );
        }
        let mut detail = String::new();
        for failure in &failures {
            detail.push_str(&format!(
                "- {} {} ({}): {}\n",
                failure.operation.name(),
                failure.id,
                failure.policy,
                failure.message
            ));
        }
        for finding in &blocking {
            detail.push_str(&format!("- {}\n", finding.message));
        }
        let violation_keys = violations(&report);
        let keys = violation_keys.keys().cloned().collect::<Vec<_>>();
        detail.push_str(&repair_instructions(
            &self.workspace,
            &state,
            &report,
            &keys,
            &violation_keys,
        ));
        self.adapter.respond_feedback(
            HookEvent::Stop,
            &Feedback {
                block: !self.provider.stop_hook_active,
                summary: "Crane policies are not satisfied; repair preserved code and complete the targets before finishing".into(),
                detail,
            },
        )
    }

    /** Report files changed since the session started that none of its recorded tool calls
     * changed: a script or program run outside the hooks, an unhooked tool, another process, or a
     * person editing at the same time. Each file is reported once per session; the stop is not
     * blocked, since the change may be legitimate, but it is recorded and alerted for review
     * Input
        - session: &SessionRecord - session
     * Output
        - Result<(), String>
    */
    fn report_unattributed(&self, session: &SessionRecord) -> Result<(), String> {
        let Ok(now) = tree(&self.workspace) else {
            return Ok(());
        };
        let found = self.with_calls(session, |calls| {
            let Some(baseline) = &calls.baseline else {
                calls.baseline = Some(now.clone());
                return Vec::new();
            };
            let found = tree_changes(baseline, &now)
                .into_iter()
                .filter(|path| {
                    !calls.attributed.contains(path) && !calls.unattributed.contains(path)
                })
                .collect::<Vec<_>>();
            calls.unattributed.extend(found.iter().cloned());
            found
        })?;
        if found.is_empty() {
            return Ok(());
        }
        let mut event = self.base_event(
            "change.unattributed",
            Domain::AuditCompliance,
            session,
            &format!(
                "unattributed:{}",
                digest("unattributed/v1", &[found.join("\n").as_bytes()])
            ),
        );
        event.severity = Severity::High;
        event.assessment = Some(Assessment::Suspicious);
        event.enforcement = Enforcement::CoverageGap;
        event.telemetry = Telemetry::Observed;
        event.reasons.push(format!(
            "{} file(s) changed during the session outside every recorded tool call (a script or program outside the hooks, an unhooked tool, another process, or a person): {}",
            found.len(),
            found.iter().take(20).cloned().collect::<Vec<_>>().join(", ")
        ));
        event.resources = found
            .iter()
            .map(|path| crate::telemetry::events::Resource {
                kind: "file".into(),
                id: path.clone(),
                access: "write".into(),
                classification: Some("unattributed".into()),
            })
            .collect();
        event.measurements.push(Measurement::observed(
            "unattributed_files",
            found.len() as f64,
            "count",
            "work tree compared with recorded tool calls",
        ));
        self.emit(session, event, Some("unattributed_change"))
    }

    /** SessionEnd: release reservations without a post-tool settlement (recorded as coverage
     * gaps), expire unused credits, close the session and task, and record a signed attestation
     * over the session's events
     * Input
        - session: &mut SessionRecord - session
     * Output
        - Result<(), String>
    */
    pub(super) fn session_end(&self, session: &mut SessionRecord) -> Result<(), String> {
        let open = self.with_calls(session, |calls| calls.calls.clone())?;
        let entries = self.stores.ledger.entries()?;
        for (call, record) in open.iter().filter(|(_, record)| record.reserved > 0) {
            let settled = entries.iter().any(|entry| {
                entry.tool_call_id.as_deref() == Some(call.as_str())
                    && matches!(entry.kind, Kind::Consume | Kind::Release)
            });
            if !settled {
                self.ledger(
                    session,
                    Some(call),
                    Kind::Release,
                    record.reserved,
                    "no post-tool settlement before the session ended",
                    &format!("{call}:release-end"),
                )?;
                let mut event = self.base_event(
                    "tool_call.unsettled",
                    Domain::AuditCompliance,
                    session,
                    &format!("{call}:unsettled"),
                );
                event.scope.tool_call_id = Some(call.clone());
                event.enforcement = Enforcement::CoverageGap;
                event.telemetry = Telemetry::Unobserved;
                event.reasons.push(format!(
                    "{} was authorized but its effect was never reported",
                    record.tool
                ));
                self.emit(session, event, None)?;
            }
        }
        let available = self.available(session)?;
        let expired = self.ledger(
            session,
            None,
            Kind::Expire,
            available,
            "session ended",
            "expire",
        )?;
        if let Some(mut task) = self.task(session) {
            if task.status == "RUNNING" {
                task.status = "ENDED".into();
                task.ended_at = Some(now_millis());
                self.stores.identity.save_task(&task)?;
            }
        }
        self.update_session(session, |session| {
            session.ended_at = Some(now_millis());
            session.end_reason = self.provider.end_reason.clone();
        })?;
        let events = self
            .stores
            .events
            .read_all()?
            .into_iter()
            .filter(|event| {
                event.scope.foxx_session_id.as_deref() == Some(session.foxx_session_id.as_str())
            })
            .collect::<Vec<_>>();
        let digests = events
            .iter()
            .map(|event| event.digest.clone())
            .collect::<Vec<_>>();
        let attestation = digest(
            "telemetry/session-attestation/v1",
            &[
                session.foxx_session_id.as_bytes(),
                digests.join("\n").as_bytes(),
            ],
        );
        // Signed with this machine's upload key: hooks never unseal a repository signing key,
        // and every teammate's machine has an upload key
        let signature = crate::trust::installation_key().ok().map(|key| {
            (
                key.public_hex(),
                key.sign("telemetry/session-attestation/v1", attestation.as_bytes()),
            )
        });
        let mut event = self.base_event(
            "session.attested",
            Domain::AuditCompliance,
            session,
            "attested",
        );
        event.measurements.push(Measurement::observed(
            "credits_expired",
            expired as f64,
            "credits",
            "autonomy ledger",
        ));
        event.measurements.push(Measurement::observed(
            "events_attested",
            digests.len() as f64,
            "count",
            "session event chain",
        ));
        // The whole session's model usage, from the agent's transcript
        if let Some(total) = self
            .provider
            .transcript
            .as_deref()
            .and_then(|path| crate::telemetry::usage::of_session(std::path::Path::new(path)))
        {
            for (name, value, unit) in [
                ("session_tokens", total.total(), "tokens"),
                ("session_input_tokens", total.input, "tokens"),
                ("session_output_tokens", total.output, "tokens"),
                ("session_cache_read_tokens", total.cache_read, "tokens"),
                (
                    "session_cache_creation_tokens",
                    total.cache_creation,
                    "tokens",
                ),
                ("session_model_turns", total.turns as f64, "count"),
                ("session_tool_calls_issued", total.calls as f64, "count"),
            ] {
                event.measurements.push(Measurement::observed(
                    name,
                    value,
                    unit,
                    "agent transcript",
                ));
            }
        }
        event.telemetry = if signature.is_some() {
            Telemetry::Observed
        } else {
            Telemetry::Unsupported
        };
        event
            .evidence
            .push(format!("attestation-sha512:{attestation}"));
        event.payload["attestation"] = json!({
            "digest": attestation,
            "events": digests.len(),
            "public_key": signature.as_ref().map(|(key, _)| key.clone()),
            "signature": signature.as_ref().map(|(_, signature)| signature.clone()),
            "signed": signature.is_some(),
            "signer": "installation",
        });
        event.payload["end_reason"] = json!(self.provider.end_reason);
        event.payload["final_safety"] = json!(session.safety);
        event.payload["session_duration_ms"] =
            json!(now_millis().saturating_sub(session.started_at));
        self.emit(session, event, None)
    }
}
