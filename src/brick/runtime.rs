// Assigned agents and the monitors that keep them cheap while idle. There is no Crane daemon and no
// reasoning loop: an assignment is a signed record, its runtime status is derived on demand from
// the local session records and events, and monitors are deterministic checks run by every
// background sync (which the hooks and commands already start) and by 'crane brick monitor run'
// (which a person or an OS scheduler can run). No check invokes a model, so idle assignments use
// no inference; Crane does not observe model tokens, so inference is reported as not measured.
// Sources without a connector (ci, metrics, external) are reported as coverage gaps, never as OK.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::compose::{attachments_of, cheap_check, check_text, controls, members};
use super::config::{self, Check, Config, Reaction, SignalSource};
use super::context::folder;
use super::model::{AssignmentState, BrickRef};
use super::store::{BrickStore, Trust};
use crate::governance::verify::{evaluate, Options, Outcome};
use crate::governance::workspace::Workspace;
use crate::platform::files::write_atomic;
use crate::telemetry::model::Decision;
use crate::telemetry::Stores;

/** A session seen within this window counts as running, in milliseconds */
const RUNNING_WINDOW_MS: u64 = 2 * 60 * 1000;

/** The outcome of a check
 * Variants
    - Proven - the evidence shows it holds
    - Contradicted - the evidence shows it does not
    - Unknown - the evidence cannot decide it
    - NotRun - it was not evaluated
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum CheckOutcome {
    Proven,
    Contradicted,
    Unknown,
    NotRun,
}

/** Evaluate a gate check (a precondition): cheap, local, and deterministic
 * Input
    - workspace: &Workspace - repository
    - check: &Check - check
    - now: u64 - Unix milliseconds
 * Output
    - Option<bool> - None for a check that is not a gate
*/
pub(crate) fn gate_holds(workspace: &Workspace, check: &Check, now: u64) -> Option<bool> {
    if let Some(holds) = cheap_check(&workspace.root, check) {
        return Some(holds);
    }
    let matches_any = |patterns: &[String], name: &str| {
        patterns
            .iter()
            .any(|pattern| crate::platform::glob::matches(pattern, name))
    };
    Some(match check {
        Check::EnvSet { name } => std::env::var_os(name).is_some_and(|value| !value.is_empty()),
        Check::BranchIs { patterns } => {
            matches_any(patterns, &crate::platform::git::branch(&workspace.root))
        }
        Check::BranchNot { patterns } => {
            !matches_any(patterns, &crate::platform::git::branch(&workspace.root))
        }
        Check::GitClean => {
            crate::platform::git::changed_files(&workspace.root).is_ok_and(|files| files.is_empty())
        }
        Check::TimeWindow {
            days,
            from,
            to,
            utc_offset_minutes,
        } => in_window(now, days, from, to, *utc_offset_minutes),
        Check::CommandAvailable { name } => on_path(name),
        Check::CredentialPresent { name } => crate::trust::vault::exists(workspace, name),
        _ => return None,
    })
}

/** Report whether a time is inside a weekly window ("from" after "to" spans midnight)
 * Input
    - now: u64 - Unix milliseconds
    - days: &[String] - weekdays (empty: every day)
    - from: &str - "HH:MM"
    - to: &str - "HH:MM"
    - offset: i32 - minutes east of UTC
 * Output
    - bool
*/
pub(crate) fn in_window(now: u64, days: &[String], from: &str, to: &str, offset: i32) -> bool {
    let (Some(start), Some(end)) = (config::minutes_of(from), config::minutes_of(to)) else {
        return false;
    };
    let local = (now / 60_000) as i64 + i64::from(offset);
    let minute = local.rem_euclid(1440) as u32;
    // 1970-01-01 was a Thursday (3 when Monday is 0)
    let weekday = (local.div_euclid(1440) + 3).rem_euclid(7) as u32;
    let day_ok = days.is_empty()
        || days
            .iter()
            .filter_map(|day| config::weekday(day))
            .any(|day| day == weekday);
    let time_ok = if start <= end {
        minute >= start && minute < end
    } else {
        minute >= start || minute < end
    };
    day_ok && time_ok
}

/** Report whether a program is on PATH
 * Input
    - name: &str - program name
 * Output
    - bool
*/
fn on_path(name: &str) -> bool {
    let suffixes: &[&str] = if cfg!(windows) {
        &["", ".exe", ".cmd", ".bat"]
    } else {
        &[""]
    };
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|directory| {
            suffixes
                .iter()
                .any(|suffix| directory.join(format!("{name}{suffix}")).is_file())
        })
    })
}

/** Run a program and wait at most a timeout (never through a shell)
 * Input
    - root: &Path - working directory
    - program: &str - program on PATH
    - args: &[String] - arguments
    - timeout: u64 - seconds
 * Output
    - Result<i32, String> exit code
*/
pub(crate) fn run_program(
    root: &Path,
    program: &str,
    args: &[String],
    timeout: u64,
) -> Result<i32, String> {
    let mut child = std::process::Command::new(program)
        .args(args)
        .current_dir(root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|error| format!("could not run {program}: {error}"))?;
    let started = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            return Ok(status.code().unwrap_or(-1));
        }
        if started.elapsed().as_secs() >= timeout {
            let _ = child.kill();
            return Err(format!("{program} ran longer than {timeout} seconds"));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/** Count local events of a target within a window
 * Input
    - runtime: &Path - runtime directory
    - target: &str - target key
    - metric: config::LocalMetric - what is counted
    - since: u64 - window start, Unix milliseconds
 * Output
    - u64
*/
fn count_events(runtime: &Path, target: &str, metric: config::LocalMetric, since: u64) -> u64 {
    let events = Stores::open(runtime).events.read_all().unwrap_or_default();
    match metric {
        config::LocalMetric::Denials => denials(&events, target, since).0,
        config::LocalMetric::ToolCalls => events
            .iter()
            .filter(|event| {
                event.occurred_at >= since
                    && event.decision.is_some()
                    && event.bindings.zones.iter().any(|entity| entity == target)
            })
            .count() as u64,
    }
}

/** Evaluate checks against the working tree, the signed state, local events, and (when a person
 * asked) endpoints and programs; the policy evaluation is run at most once and only when needed
 * Input
    - workspace: &Workspace - repository
    - checks: &[Check] - checks
    - target: Option<&str> - the target the checks are for (local metrics)
    - person: bool - a person asked (programs and endpoints run only then)
 * Output
    - Vec<(String, CheckOutcome, String)> check, outcome, detail
*/
pub(crate) fn run_checks(
    workspace: &Workspace,
    checks: &[Check],
    target: Option<&str>,
    person: bool,
) -> Vec<(String, CheckOutcome, String)> {
    let mut report = None;
    let now = crate::platform::now_millis();
    let outcome = |holds: bool| {
        if holds {
            CheckOutcome::Proven
        } else {
            CheckOutcome::Contradicted
        }
    };
    checks
        .iter()
        .map(|check| {
            let text = check_text(check);
            match check {
                Check::SignedState => match super::store::load(workspace) {
                    Ok((state, _, _)) => match super::store::trust_of(workspace, &state) {
                        Trust::Unverified(reason) => (text, CheckOutcome::Contradicted, reason),
                        _ => {
                            let blocking = workspace
                                .trust()
                                .map(|trust| {
                                    crate::governance::state::integrity(workspace, &state, &trust)
                                        .into_iter()
                                        .filter(crate::governance::Finding::blocking)
                                        .count()
                                })
                                .unwrap_or(1);
                            if blocking == 0 {
                                (text, CheckOutcome::Proven, String::new())
                            } else {
                                (
                                    text,
                                    CheckOutcome::Contradicted,
                                    format!("{blocking} blocking integrity finding(s)"),
                                )
                            }
                        }
                    },
                    Err(error) => (text, CheckOutcome::Unknown, error),
                },
                Check::SelectionsResolved | Check::PolicyTestsPass => {
                    if report.is_none() {
                        report = Some(
                            evaluate(workspace, &Options::default()).map(|(report, _)| report),
                        );
                    }
                    match report.as_ref() {
                        Some(Ok(report)) => {
                            let commands =
                                report.policies.iter().flat_map(|policy| &policy.commands);
                            let bad = commands
                                .filter(|command| match check {
                                    Check::SelectionsResolved => {
                                        command.outcome == Outcome::Unresolved
                                    }
                                    _ => matches!(
                                        command.outcome,
                                        Outcome::Fail | Outcome::Unresolved
                                    ),
                                })
                                .count();
                            if bad == 0 {
                                (text, CheckOutcome::Proven, String::new())
                            } else {
                                (
                                    text,
                                    CheckOutcome::Contradicted,
                                    format!("{bad} command(s) failing or unresolved"),
                                )
                            }
                        }
                        Some(Err(error)) => (text, CheckOutcome::Unknown, error.clone()),
                        None => (text, CheckOutcome::NotRun, String::new()),
                    }
                }
                Check::HttpOk { url, status, .. } if person => {
                    match crate::platform::http::request("GET", url, &[], None) {
                        Ok((code, _)) => {
                            let holds =
                                status.map_or((200..300).contains(&code), |wanted| wanted == code);
                            (text, outcome(holds), format!("answered {code}"))
                        }
                        Err(error) => (text, CheckOutcome::Contradicted, error),
                    }
                }
                Check::HttpOk { .. } => (
                    text,
                    CheckOutcome::NotRun,
                    "endpoints are checked by 'crane brick test', run by a person".into(),
                ),
                Check::JsonValue {
                    path,
                    pointer,
                    op,
                    value,
                } => {
                    let found = std::fs::read_to_string(workspace.root.join(path))
                        .map_err(|error| format!("{path}: {error}"))
                        .and_then(|text| {
                            serde_json::from_str::<Value>(&text)
                                .map_err(|error| format!("{path}: {error}"))
                        })
                        .and_then(|json| {
                            json.pointer(pointer)
                                .and_then(Value::as_f64)
                                .ok_or_else(|| format!("{path} has no number at {pointer}"))
                        });
                    match (found, value.as_f64()) {
                        (Ok(actual), Some(expected)) => {
                            let holds = match op {
                                config::Comparison::AtLeast => actual >= expected,
                                config::Comparison::AtMost => actual <= expected,
                                config::Comparison::Equals => {
                                    (actual - expected).abs() < f64::EPSILON
                                }
                            };
                            (text, outcome(holds), format!("found {actual}"))
                        }
                        (Err(error), _) => (text, CheckOutcome::Unknown, error),
                        (_, None) => (
                            text,
                            CheckOutcome::Unknown,
                            "the expected value is not a number".into(),
                        ),
                    }
                }
                Check::LocalMetric {
                    metric,
                    window_hours,
                    max,
                } => match (target, workspace.trust()) {
                    (Some(target), Ok(trust)) => {
                        let since = now.saturating_sub(u64::from(*window_hours) * 3_600_000);
                        let count = count_events(&trust.runtime(), target, *metric, since);
                        (
                            text,
                            outcome(count <= *max),
                            format!("{count} in {window_hours} h"),
                        )
                    }
                    (None, _) => (
                        text,
                        CheckOutcome::NotRun,
                        "local metrics are measured for a target ('crane brick test --target')"
                            .into(),
                    ),
                    (_, Err(error)) => (text, CheckOutcome::Unknown, error),
                },
                Check::CommandSucceeds {
                    program,
                    args,
                    timeout_seconds,
                } if person => {
                    let timeout = u64::from(timeout_seconds.unwrap_or(120));
                    match run_program(&workspace.root, program, args, timeout) {
                        Ok(code) => (text, outcome(code == 0), format!("exit code {code}")),
                        Err(error) => (text, CheckOutcome::Contradicted, error),
                    }
                }
                Check::CommandSucceeds { .. } => (
                    text,
                    CheckOutcome::NotRun,
                    "programs run only in 'crane brick test', run by a person".into(),
                ),
                gate => match gate_holds(workspace, gate, now) {
                    Some(holds) => (text, outcome(holds), String::new()),
                    None => (text, CheckOutcome::NotRun, String::new()),
                },
            }
        })
        .collect()
}

/** One monitor's remembered state
 * Fields
    - last_run: u64 - Unix milliseconds
    - result: String - OK, FIRING, FAILING, UNSUPPORTED, REVOCATION_DUE, PROPOSAL, ERROR
    - detail: String - what it found
*/
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct MonitorState {
    last_run: u64,
    result: String,
    detail: String,
}

/** Read the monitor states
 * Input
    - runtime: &Path - runtime directory
 * Output
    - BTreeMap<String, MonitorState>
*/
fn read_states(runtime: &Path) -> BTreeMap<String, MonitorState> {
    fs::read_to_string(folder(runtime).join("monitors.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/** One monitored item of the store
 * Fields
    - key: String - stable monitor key
    - target: String - target key
    - brick: String - Brick reference
    - kind: &'static str - signal, trigger, or analysis
    - name: String - source, trigger, or analysis name
    - every_minutes: u32 - interval
    - threshold: Option<u64> - count threshold
    - reactions: Vec<Reaction> - what a firing signal does
*/
#[derive(Debug, Clone)]
struct Item {
    key: String,
    target: String,
    brick: String,
    kind: &'static str,
    name: String,
    every_minutes: u32,
    threshold: Option<u64>,
    reactions: Vec<Reaction>,
}

/** The readable configurations of a target, with the reference of the Brick each came from
 * Input
    - store: &BrickStore - signed store
    - target: &str - target key
 * Output
    - Vec<(BrickRef, Config)>
*/
fn target_configs(store: &BrickStore, target: &str) -> Vec<(BrickRef, Config)> {
    let mut parsed = Vec::new();
    for attachment in attachments_of(store, target) {
        for member in members(store, &attachment.subject) {
            if let Some(entry) = member.entry {
                if let Ok(configs) = config::parse(entry.brick.brick_type, &entry.brick.config) {
                    parsed.extend(
                        configs
                            .into_iter()
                            .map(|config| (entry.brick.reference(), config)),
                    );
                }
            }
        }
    }
    parsed
}

/** Every monitored item of the enabled attachments: watch signals, triggers, and analyses (the
 * last two on the interval of the Brick's first signal, or hourly)
 * Input
    - store: &BrickStore - signed store
 * Output
    - Vec<Item>
*/
fn items(store: &BrickStore) -> Vec<Item> {
    let mut items = Vec::new();
    let mut targets = store
        .attachments
        .iter()
        .map(|attachment| attachment.target.key())
        .collect::<Vec<_>>();
    targets.sort();
    targets.dedup();
    for target in targets {
        let parsed = target_configs(store, &target);
        let watches = parsed
            .iter()
            .filter_map(|(reference, config)| match config {
                Config::Watch(watch) => Some((reference, watch)),
                _ => None,
            })
            .collect::<Vec<_>>();
        let interval = watches
            .iter()
            .find_map(|(_, watch)| watch.signals.first().map(|signal| signal.every_minutes))
            .unwrap_or(60);
        let threshold = watches.iter().find_map(|(_, watch)| {
            watch
                .signals
                .iter()
                .find(|signal| signal.source == SignalSource::Denials)
                .and_then(|signal| signal.threshold)
        });
        for (reference, watch) in watches {
            let mut push = |kind: &'static str, name: String, every: u32, limit: Option<u64>| {
                items.push(Item {
                    key: format!("{target}|{reference}|{kind}|{name}"),
                    target: target.clone(),
                    brick: reference.to_string(),
                    kind,
                    name,
                    every_minutes: every,
                    threshold: limit,
                    reactions: if kind == "signal" {
                        watch.on_fire.clone()
                    } else {
                        Vec::new()
                    },
                });
            };
            for signal in &watch.signals {
                push(
                    "signal",
                    config::serial(&signal.source),
                    signal.every_minutes,
                    signal.threshold,
                );
            }
            for trigger in &watch.triggers {
                push(
                    "trigger",
                    config::serial(trigger),
                    interval,
                    threshold.or(Some(5)),
                );
            }
            for analysis in &watch.analyses {
                push(
                    "analysis",
                    config::serial(analysis),
                    interval.max(60),
                    Some(3),
                );
            }
        }
    }
    items
}

/** The Watch Bricks whose signals fire now, by target (what 'pause' acts on)
 * Input
    - runtime: &Path - runtime directory
 * Output
    - BTreeSet<(String, String)> (target, Brick reference) pairs
*/
pub(crate) fn firing(runtime: &Path) -> std::collections::BTreeSet<(String, String)> {
    read_states(runtime)
        .into_iter()
        .filter(|(_, state)| matches!(state.result.as_str(), "FIRING" | "FAILING"))
        .filter_map(|(key, _)| {
            let mut parts = key.split('|');
            let (target, brick, kind) = (parts.next()?, parts.next()?, parts.next()?);
            (kind == "signal").then(|| (target.to_string(), brick.to_string()))
        })
        .collect()
}

/** Count denied agent actions bound to a target within a window
 * Input
    - events: &[crate::telemetry::events::Event] - local events
    - target: &str - target key
    - since: u64 - window start, Unix milliseconds
 * Output
    - (u64, BTreeMap<String, u64>) denials and their count by rule
*/
pub(crate) fn denials(
    events: &[crate::telemetry::events::Event],
    target: &str,
    since: u64,
) -> (u64, BTreeMap<String, u64>) {
    let mut total = 0;
    let mut rules = BTreeMap::new();
    for event in events.iter().filter(|event| {
        event.occurred_at >= since
            && matches!(event.decision, Some(Decision::Deny | Decision::Quarantine))
            && event.bindings.zones.iter().any(|entity| entity == target)
    }) {
        total += 1;
        for rule in event.payload["rules"].as_array().into_iter().flatten() {
            if let Some(rule) = rule.as_str() {
                *rules.entry(rule.to_string()).or_default() += 1;
            }
        }
    }
    (total, rules)
}

/** Evaluate the monitors that are due (or all of them when forced) and remember their results;
 * each evaluation is noted for the event the running command or sync records next (F37)
 * Input
    - workspace: &Workspace - repository
    - runtime: &Path - runtime directory
    - force: bool - evaluate every monitor now
 * Output
    - Result<Vec<Value>, String> evaluations
*/
pub(crate) fn evaluate_monitors(
    workspace: &Workspace,
    runtime: &Path,
    force: bool,
) -> Result<Vec<Value>, String> {
    let (state, store, trust) = super::store::load(workspace)?;
    let items = items(&store);
    if items.is_empty() {
        return Ok(Vec::new());
    }
    let now = crate::platform::now_millis();
    let mut states = read_states(runtime);
    let mut events = None;
    let mut report = None;
    let mut results = Vec::new();
    for item in items {
        let previous = states.get(&item.key).cloned().unwrap_or_default();
        let due = force
            || now.saturating_sub(previous.last_run) >= u64::from(item.every_minutes) * 60_000;
        if !due {
            continue;
        }
        let window = now.saturating_sub(u64::from(item.every_minutes.max(5)) * 60_000);
        let mut events_since = |since: u64, target: &str| {
            let all = events
                .get_or_insert_with(|| Stores::open(runtime).events.read_all().unwrap_or_default());
            denials(all, target, since)
        };
        let mut policy = |check: Check| {
            report.get_or_insert_with(|| {
                run_checks(
                    workspace,
                    &[Check::SelectionsResolved, Check::PolicyTestsPass],
                    None,
                    false,
                )
            });
            let index = usize::from(check == Check::PolicyTestsPass);
            report.as_ref().and_then(|found| found.get(index).cloned())
        };
        let integrity_ok = || match &trust {
            Trust::Unverified(_) => false,
            _ => workspace
                .trust()
                .map(|trust| {
                    !crate::governance::state::integrity(workspace, &state, &trust)
                        .iter()
                        .any(crate::governance::Finding::blocking)
                })
                .unwrap_or(false),
        };
        let (result, detail) = match (item.kind, item.name.as_str()) {
            ("signal", "governance_integrity") | ("trigger", "integrity_failure") => {
                if integrity_ok() {
                    ("OK", "the signed state verifies".to_string())
                } else if item.kind == "trigger" {
                    (
                        "REVOCATION_DUE",
                        "the signed state does not verify; an admin should review the attachments"
                            .into(),
                    )
                } else {
                    ("FAILING", "the signed state does not verify".into())
                }
            }
            ("signal", "selections_resolved") | ("signal", "policy_tests") => {
                let check = if item.name == "policy_tests" {
                    Check::PolicyTestsPass
                } else {
                    Check::SelectionsResolved
                };
                match policy(check) {
                    Some((_, CheckOutcome::Proven, _)) => ("OK", "holds".into()),
                    Some((_, CheckOutcome::Contradicted, detail)) => ("FAILING", detail),
                    Some((_, _, detail)) => ("ERROR", detail),
                    None => ("ERROR", "not evaluated".into()),
                }
            }
            ("trigger", "verification_contradicted") | ("signal", "verification") => {
                let checks = verification_checks(&store, &item.target);
                if checks.is_empty() {
                    (
                        "UNSUPPORTED",
                        "no verification Brick is attached to the target".into(),
                    )
                } else {
                    let contradicted = run_checks(workspace, &checks, Some(&item.target), false)
                        .into_iter()
                        .filter(|(_, outcome, _)| *outcome == CheckOutcome::Contradicted)
                        .map(|(check, _, _)| check)
                        .collect::<Vec<_>>();
                    if contradicted.is_empty() {
                        ("OK", "every verification check holds".into())
                    } else if item.kind == "trigger" {
                        (
                            "REVOCATION_DUE",
                            format!("contradicted: {}", contradicted.join("; ")),
                        )
                    } else {
                        (
                            "FAILING",
                            format!("contradicted: {}", contradicted.join("; ")),
                        )
                    }
                }
            }
            ("signal", "preconditions") => {
                let failing = target_configs(&store, &item.target)
                    .into_iter()
                    .filter_map(|(_, config)| match config {
                        Config::Precondition(precondition) => Some(precondition.checks),
                        _ => None,
                    })
                    .flatten()
                    .filter(|check| gate_holds(workspace, check, now) == Some(false))
                    .map(|check| check_text(&check))
                    .collect::<Vec<_>>();
                if failing.is_empty() {
                    ("OK", "every precondition holds".into())
                } else {
                    ("FAILING", format!("failing: {}", failing.join("; ")))
                }
            }
            ("signal", "denials") | ("trigger", "repeated_denials") => {
                let (count, _) = events_since(window, &item.target);
                let threshold = item.threshold.unwrap_or(5);
                if count >= threshold {
                    (
                        if item.kind == "trigger" {
                            "REVOCATION_DUE"
                        } else {
                            "FIRING"
                        },
                        format!(
                            "{count} denials in {} minutes (threshold {threshold})",
                            item.every_minutes
                        ),
                    )
                } else {
                    (
                        "OK",
                        format!("{count} denials in {} minutes", item.every_minutes),
                    )
                }
            }
            ("analysis", "denial_patterns") => {
                let (_, rules) = events_since(now.saturating_sub(24 * 3_600_000), &item.target);
                let repeated = rules
                    .iter()
                    .filter(|(_, count)| **count >= item.threshold.unwrap_or(3))
                    .map(|(rule, count)| format!("{rule} ({count}x)"))
                    .collect::<Vec<_>>();
                if repeated.is_empty() {
                    ("OK", "no repeated denials in 24 hours".into())
                } else {
                    ("PROPOSAL", format!("agents were repeatedly denied by {}; consider adding Current Context for {} (a proposal only; nothing was changed)", repeated.join(", "), item.target))
                }
            }
            ("analysis", "stale_context") => {
                let stale = attachments_of(&store, &item.target)
                    .into_iter()
                    .filter(|attachment| {
                        attachment.current_context.updated_at > 0
                            && now.saturating_sub(attachment.current_context.updated_at)
                                > 30 * 86_400_000
                    })
                    .map(|attachment| attachment.attachment_id.clone())
                    .collect::<Vec<_>>();
                if stale.is_empty() {
                    ("OK", "Current Context was reviewed within 30 days".into())
                } else {
                    ("PROPOSAL", format!("Current Context of {} is older than 30 days; review it (a proposal only)", stale.join(", ")))
                }
            }
            (_, source) => (
                "UNSUPPORTED",
                format!("no connector for {source} exists in this version; this is a coverage gap"),
            ),
        };
        crate::platform::notes::note(
            "BRICK_MONITOR_EVALUATED",
            json!({"target": item.target, "brick": item.brick, "monitor": item.kind, "source": item.name, "result": result, "inference": "NONE"}),
        );
        // A signal that starts firing reacts once (alert, revoke due, propose); pause is read by
        // the guard for as long as it fires
        let fires = matches!(result, "FIRING" | "FAILING");
        let fired_before = matches!(previous.result.as_str(), "FIRING" | "FAILING");
        if fires && !fired_before {
            for reaction in &item.reactions {
                let text = format!(
                    "Crane watch {} on {}: {} {}: {}",
                    item.brick, item.target, item.name, result, detail
                );
                match reaction {
                    Reaction::Alert => {
                        crate::integrations::deliver(
                            workspace,
                            &crate::integrations::Alert {
                                text: &text,
                                severity: 2,
                                issue: None,
                            },
                        );
                    }
                    Reaction::RevokeDue | Reaction::Propose | Reaction::Pause => {}
                }
                crate::platform::notes::note(
                    "BRICK_WATCH_REACTED",
                    json!({"target": item.target, "brick": item.brick, "source": item.name, "reaction": config::serial(reaction), "result": result}),
                );
            }
        }
        states.insert(
            item.key.clone(),
            MonitorState {
                last_run: now,
                result: result.into(),
                detail: detail.clone(),
            },
        );
        results.push(json!({
            "target": item.target, "brick": item.brick, "monitor": item.kind, "source": item.name,
            "result": result, "detail": detail, "at": now, "inference": "none",
        }));
    }
    fs::create_dir_all(folder(runtime)).map_err(crate::platform::io_error)?;
    write_atomic(
        &folder(runtime).join("monitors.json"),
        serde_json::to_string(&states)
            .map_err(crate::platform::io_error)?
            .as_bytes(),
    )?;
    Ok(results)
}

/** The remembered monitor results, with their next due time
 * Input
    - workspace: &Workspace - repository
    - runtime: &Path - runtime directory
 * Output
    - Vec<Value>
*/
pub(crate) fn monitor_status(workspace: &Workspace, runtime: &Path) -> Vec<Value> {
    let Ok((_, store, _)) = super::store::load(workspace) else {
        return Vec::new();
    };
    let states = read_states(runtime);
    items(&store)
        .into_iter()
        .map(|item| {
            let state = states.get(&item.key);
            json!({
                "target": item.target, "brick": item.brick, "monitor": item.kind, "source": item.name,
                "every_minutes": item.every_minutes,
                "supported": !matches!(item.name.as_str(), "ci" | "metrics" | "external"),
                "last_run": state.map(|state| state.last_run),
                "result": state.map_or("NOT_RUN", |state| state.result.as_str()),
                "detail": state.map(|state| state.detail.clone()),
                "next_due": state.map(|state| state.last_run + u64::from(item.every_minutes) * 60_000),
            })
        })
        .collect()
}

/** Evaluate the due monitors from the background sync and record one event for them (the sync
 * stores no other event, so the notes of the evaluations attach to this one); best effort
 * Input
    - workspace: &Workspace - repository
    - stores: &Stores - runtime stores
 * Output
    - None
*/
pub(crate) fn sync_monitors(workspace: &Workspace, stores: &Stores) {
    if !workspace
        .file(crate::governance::state::BRICKS_FILE)
        .exists()
    {
        return;
    }
    let now = crate::platform::now_millis();
    let (count, attention, error) = match evaluate_monitors(workspace, &stores.directory, false) {
        Ok(results) if results.is_empty() => return,
        Ok(results) => {
            let attention = results
                .iter()
                .filter(|result| result["result"] != "OK")
                .count();
            (results.len(), attention, None)
        }
        Err(error) => (0, 0, Some(error)),
    };
    let mut event = crate::telemetry::events::Event::new(
        "brick.monitors_evaluated",
        crate::telemetry::model::Domain::RecoveryOversight,
        &workspace.repository_id,
        &format!("brick-monitors:{now}"),
    );
    event.payload = json!({"evaluated": count, "attention": attention, "trigger": "SYNC", "error": error, "inference": "NONE"});
    if let Some(error) = error {
        event.reasons.push(error);
    }
    let _ = stores.events.append(event);
}

/** The status of every assignment: its signed state, the runtime status derived from local
 * sessions, events, and monitors, and what is not measured
 * Input
    - workspace: &Workspace - repository
    - runtime: &Path - runtime directory
 * Output
    - Result<Vec<Value>, String>
*/
pub(crate) fn assignments(workspace: &Workspace, runtime: &Path) -> Result<Vec<Value>, String> {
    let (_, store, trust) = super::store::load(workspace)?;
    let stores = Stores::open(runtime);
    let sessions = stores.identity.sessions();
    let monitors = monitor_status(workspace, runtime);
    let now = crate::platform::now_millis();
    let mut events = None;
    let mut rows = Vec::new();
    for assignment in &store.assignments {
        let mine = sessions
            .iter()
            .filter(|session| {
                session.provider == assignment.agent || session.agent_id == assignment.agent
            })
            .collect::<Vec<_>>();
        let live = mine.iter().find(|session| {
            session.ended_at.is_none()
                && now.saturating_sub(session.last_seen_at) <= RUNNING_WINDOW_MS
        });
        let targets = assignment
            .targets
            .iter()
            .map(super::model::Target::key)
            .collect::<Vec<_>>();
        let mut problems = Vec::new();
        for target in &targets {
            let combined = controls(&store, target, &workspace.root);
            problems.extend(
                combined
                    .fail_closed
                    .iter()
                    .map(|pin| format!("{target}: {pin}")),
            );
        }
        problems.extend(
            monitors
                .iter()
                .filter(|monitor| {
                    targets
                        .iter()
                        .any(|target| monitor["target"] == target.as_str())
                })
                .filter(|monitor| {
                    matches!(
                        monitor["result"].as_str(),
                        Some("UNSUPPORTED" | "FAILING" | "ERROR" | "REVOCATION_DUE")
                    )
                })
                .map(|monitor| {
                    format!(
                        "{} {} {}: {}",
                        monitor["target"].as_str().unwrap_or_default(),
                        monitor["source"].as_str().unwrap_or_default(),
                        monitor["result"].as_str().unwrap_or_default(),
                        monitor["detail"].as_str().unwrap_or_default()
                    )
                }),
        );
        let waiting = live.is_some_and(|session| {
            let all = events.get_or_insert_with(|| stores.events.read_all().unwrap_or_default());
            all.iter()
                .rev()
                .find(|event| {
                    event.scope.foxx_session_id.as_deref() == Some(session.foxx_session_id.as_str())
                        && event.decision.is_some()
                })
                .is_some_and(|event| event.decision == Some(Decision::RequireApproval))
        });
        let runtime_status = match assignment.state {
            AssignmentState::Revoked => "REVOKED",
            AssignmentState::Paused => "PAUSED",
            AssignmentState::Active if matches!(trust, Trust::Unverified(_)) => "ERROR",
            AssignmentState::Active if waiting => "WAITING_FOR_APPROVAL",
            AssignmentState::Active if live.is_some() => "RUNNING",
            AssignmentState::Active if !problems.is_empty() => "DEGRADED",
            AssignmentState::Active => "IDLE",
        };
        rows.push(json!({
            "agent": assignment.agent,
            "assignment_status": assignment.state,
            "runtime_status": runtime_status,
            "targets": targets,
            "assigned_by": assignment.assigned_by.name,
            "assigned_at": assignment.assigned_at,
            "sessions": mine.len(),
            "last_seen_at": mine.first().map(|session| session.last_seen_at),
            "live_session": live.map(|session| session.foxx_session_id.clone()),
            "problems": problems,
            "inference": {"status": "not_measured", "idle_inference": "none: monitors are deterministic and never call a model", "note": "Crane does not observe model tokens"},
        }));
    }
    Ok(rows)
}

/** Check whether an agent is admitted by the identity Bricks of its targets
 * Input
    - store: &BrickStore - signed store
    - target_key: &str - target
    - agent: &str - provider or agent id
 * Output
    - Option<(bool, Vec<String>)> - None when no identity Brick applies; else admitted and the
      Bricks that decided
*/
pub(crate) fn identity_admits(
    store: &BrickStore,
    target_key: &str,
    agent: &str,
) -> Option<(bool, Vec<String>)> {
    let mut deciding = Vec::new();
    let mut admitted = true;
    for (reference, config) in target_configs(store, target_key) {
        if let Config::Identity(identity) = config {
            deciding.push(reference.to_string());
            admitted &= identity
                .agents
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(agent));
        }
    }
    (!deciding.is_empty()).then_some((admitted, deciding))
}

/** The verification checks of a target: each Verification Brick's own checks, and the checks of
 * the Precondition Brick it re-runs after the work (checks_from)
 * Input
    - store: &BrickStore - signed store
    - target: &str - target key
 * Output
    - Vec<Check>
*/
pub(crate) fn verification_checks(store: &BrickStore, target: &str) -> Vec<Check> {
    let configs = target_configs(store, target);
    let mut checks = Vec::new();
    for (_, config) in &configs {
        if let Config::Verification(verification) = config {
            checks.extend(verification.checks.iter().cloned());
            if let Some(source) = &verification.checks_from {
                checks.extend(
                    configs
                        .iter()
                        .filter(|(reference, _)| reference.brick_id == *source)
                        .filter_map(|(_, config)| match config {
                            Config::Precondition(precondition) => Some(precondition.checks.clone()),
                            _ => None,
                        })
                        .flatten(),
                );
            }
        }
    }
    checks
}
