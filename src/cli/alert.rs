// Alert rules from the command line (telemetry/rules.rs): add, list, remove, enable or disable,
// evaluate now, send a test notification, read the log, and list the metrics. Rules are built here
// and nowhere else; the dashboard only shows them, their evaluations, and their deliveries.
// Changing rules or sending notifications is refused to agents.

use serde_json::{json, Value};

use crate::governance::workspace::Workspace;
use crate::platform::files::read_lines;
use crate::platform::{actor, now_millis};
use crate::telemetry::query::{facet_path, Clause, Op};
use crate::telemetry::rules::{self, Rule, COMPARISONS, METRICS};
use crate::telemetry::Stores;
use crate::trust::require_human;

/** How to use 'crane alert' */
const USAGE: &str = "usage:
  crane alert add NAME metric METRIC above|at_least|below|at_most|equals N [window MINUTES|15m|2h|1d]
                  [channels slack,whatsapp] [severity low|medium|high|critical] [cooldown MINUTES]
                  [where PATH is|not|contains|not-contains|prefix|suffix|like|gt|gte|lt|lte VALUE[,VALUE...]]...
                  [where PATH between LOW,HIGH]... [where PATH exists|missing]...
                  [except PATH OPERATOR VALUES]... [quiet-resolve]
                  (where counts only events that meet the condition, except only those that do not;
                  like takes patterns with * and ?)
  crane alert list [--json]
  crane alert remove NAME
  crane alert enable NAME | crane alert disable NAME
  crane alert check [--json]
  crane alert test NAME
  crane alert log [--json]
  crane alert metrics";

/** Dispatch an alert command
 * Input
    - args: &[String] - arguments after "alert"
 * Output
    - Result<(), String>
*/
pub(crate) fn alert(args: &[String]) -> Result<(), String> {
    let json = args.iter().any(|arg| arg == "--json");
    match args.first().map(String::as_str) {
        Some("add") => add(&args[1..]),
        Some("list") => list(json),
        Some("remove") => change(args.get(1), "remove"),
        Some("enable") => change(args.get(1), "enable"),
        Some("disable") => change(args.get(1), "disable"),
        Some("check") => check(json),
        Some("test") => test(args.get(1)),
        Some("log") => log(json),
        Some("metrics") => {
            println!(
                "Metrics an alert rule can watch (counted over the rule's window and filter):"
            );
            for (name, meaning) in METRICS {
                println!("  {name:<26} {meaning}");
            }
            Ok(())
        }
        _ => Err(USAGE.into()),
    }
}

/** Read a window or cooldown: minutes, or a number with m, h, or d
 * Input
    - text: &str - value
 * Output
    - Result<u64, String> minutes
*/
fn minutes(text: &str) -> Result<u64, String> {
    let (number, unit) = text.trim().split_at(
        text.trim()
            .find(|character: char| !character.is_ascii_digit())
            .unwrap_or(text.trim().len()),
    );
    let value = number
        .parse::<u64>()
        .map_err(|_| format!("'{text}' is not a duration such as 15, 15m, 2h, or 1d"))?;
    match unit {
        "" | "m" => Ok(value),
        "h" => Ok(value * 60),
        "d" => Ok(value * 1440),
        _ => Err(format!(
            "'{text}' is not a duration such as 15, 15m, 2h, or 1d"
        )),
    }
}

/** Read a filter operator word
 * Input
    - word: &str - operator
 * Output
    - Option<Op>
*/
fn operator(word: &str) -> Option<Op> {
    Some(match word {
        "is" => Op::Is,
        "not" => Op::Not,
        "contains" => Op::Contains,
        "not-contains" => Op::NotContains,
        "prefix" => Op::Prefix,
        "suffix" => Op::Suffix,
        "like" => Op::Like,
        "between" => Op::Between,
        "gt" => Op::Gt,
        "gte" => Op::Gte,
        "lt" => Op::Lt,
        "lte" => Op::Lte,
        "exists" => Op::Exists,
        "missing" => Op::Missing,
        _ => return None,
    })
}

/** Parse the words of 'crane alert add' into a rule
 * Input
    - args: &[String] - NAME and its settings
    - by: String - who adds it
    - now: u64 - Unix milliseconds
 * Output
    - Result<Rule, String>
*/
pub(crate) fn parse(args: &[String], by: String, now: u64) -> Result<Rule, String> {
    let name = args.first().ok_or(USAGE)?.clone();
    let mut rule = Rule {
        name,
        metric: String::new(),
        op: String::new(),
        threshold: f64::NAN,
        window_minutes: 15,
        channels: Vec::new(),
        severity: "HIGH".into(),
        cooldown_minutes: 30,
        filter: Vec::new(),
        enabled: true,
        notify_resolved: true,
        created_at: now,
        created_by: by,
    };
    let mut index = 1;
    let next = |index: &mut usize, what: &str| -> Result<String, String> {
        *index += 1;
        args.get(*index)
            .cloned()
            .ok_or_else(|| format!("{what} needs a value"))
    };
    while index < args.len() {
        let word = args[index].to_ascii_lowercase().replace('-', "_");
        match word.as_str() {
            "metric" => rule.metric = next(&mut index, "metric")?,
            "window" => rule.window_minutes = minutes(&next(&mut index, "window")?)?,
            "cooldown" => rule.cooldown_minutes = minutes(&next(&mut index, "cooldown")?)?,
            "severity" => rule.severity = next(&mut index, "severity")?.to_ascii_uppercase(),
            "channels" | "channel" => {
                rule.channels = next(&mut index, "channels")?
                    .split(',')
                    .map(|channel| channel.trim().to_ascii_lowercase())
                    .filter(|channel| !channel.is_empty())
                    .collect()
            }
            "quiet_resolve" => rule.notify_resolved = false,
            "where" | "except" => {
                let exclude = word == "except";
                let path = next(&mut index, &word)?;
                let path = facet_path(&path).map_or(path, String::from);
                let op_word = next(&mut index, &format!("{word} PATH"))?.to_ascii_lowercase();
                let op = operator(&op_word).ok_or_else(|| format!("unknown operator '{op_word}'; use is, not, contains, not-contains, prefix, suffix, like, gt, gte, lt, lte, between, exists, or missing"))?;
                let values = if matches!(op, Op::Exists | Op::Missing) {
                    Vec::new()
                } else {
                    next(&mut index, "the operator")?
                        .split(',')
                        .map(|value| value.trim().to_string())
                        .filter(|value| !value.is_empty())
                        .collect()
                };
                if op == Op::Between && values.len() != 2 {
                    return Err("between needs two numbers, such as: between 100,500".into());
                }
                rule.filter.push(Clause {
                    path,
                    op,
                    values,
                    group: None,
                    exclude,
                });
            }
            comparison if COMPARISONS.iter().any(|(name, _)| *name == comparison) => {
                rule.op = comparison.into();
                let text = next(&mut index, comparison)?;
                rule.threshold = text
                    .parse::<f64>()
                    .map_err(|_| format!("'{text}' is not a number"))?;
            }
            other => return Err(format!("unknown setting '{other}'\n{USAGE}")),
        }
        index += 1;
    }
    if rule.channels.is_empty() {
        rule.channels = vec!["slack".into()];
    }
    if rule.metric.is_empty() || rule.op.is_empty() {
        return Err(format!(
            "a rule needs a metric and a comparison, such as: metric denied above 5\n{USAGE}"
        ));
    }
    rules::check(&rule)?;
    Ok(rule)
}

/** Add or replace a rule
 * Input
    - args: &[String] - NAME and its settings
 * Output
    - Result<(), String>
*/
fn add(args: &[String]) -> Result<(), String> {
    require_human("crane alert add")?;
    let workspace = Workspace::locate()?;
    let rule = parse(
        args,
        workspace.user.clone().unwrap_or_else(actor),
        now_millis(),
    )?;
    let mut all = rules::load(&workspace);
    let replaced = all.iter().any(|existing| existing.name == rule.name);
    all.retain(|existing| existing.name != rule.name);
    all.push(rule.clone());
    rules::save(&workspace, &all)?;
    let missing = rule
        .channels
        .iter()
        .filter(|channel| crate::integrations::load(&workspace, channel).is_none())
        .cloned()
        .collect::<Vec<_>>();
    println!(
        "{} alert rule '{}': {}",
        if replaced { "Replaced" } else { "Added" },
        rule.name,
        rule.describe()
    );
    println!(
        "Notifies {} at severity {}, at most every {} min while firing{}.",
        rule.channels.join(" and "),
        rule.severity,
        rule.cooldown_minutes,
        if rule.notify_resolved {
            ", and when it resolves"
        } else {
            ""
        }
    );
    if !missing.is_empty() {
        println!("Not configured yet: {}. Run 'crane integrate {}' first, or deliveries will be recorded as failed.", missing.join(", "), missing[0]);
    }
    println!(
        "Rules are evaluated by every background sync; run 'crane alert check' to evaluate now."
    );
    Ok(())
}

/** List the rules with their state
 * Input
    - json: bool - print JSON
 * Output
    - Result<(), String>
*/
fn list(json: bool) -> Result<(), String> {
    let workspace = Workspace::locate()?;
    let stores = Stores::open(&workspace.trust()?.runtime());
    let report = rules::report(&workspace, &stores);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).unwrap_or_default()
        );
        return Ok(());
    }
    let rules = rules::load(&workspace);
    if rules.is_empty() {
        println!("No alert rules. Add one with: crane alert add NAME metric denied above 5 window 15m channels slack");
        return Ok(());
    }
    for rule in rules {
        let state = &report["state"][&rule.name];
        println!(
            "{:<24} {:<8} {:<9} {}  →  {}{}",
            rule.name,
            if rule.enabled {
                state["state"].as_str().unwrap_or("NEW")
            } else {
                "DISABLED"
            },
            rule.severity,
            rule.describe(),
            rule.channels.join(","),
            state["last_value"]
                .as_f64()
                .map(|value| format!("  (last value {value})"))
                .unwrap_or_default()
        );
    }
    for channel in report["channels"].as_array().into_iter().flatten() {
        if channel["configured"] == false {
            println!(
                "Note: {} is not configured ('crane integrate {}').",
                channel["channel"].as_str().unwrap_or_default(),
                channel["channel"].as_str().unwrap_or_default()
            );
        }
    }
    Ok(())
}

/** Remove, enable, or disable a rule
 * Input
    - name: Option<&String> - rule
    - action: &str - remove, enable, or disable
 * Output
    - Result<(), String>
*/
fn change(name: Option<&String>, action: &str) -> Result<(), String> {
    require_human(&format!("crane alert {action}"))?;
    let name = name.ok_or(USAGE)?;
    let workspace = Workspace::locate()?;
    let mut all = rules::load(&workspace);
    if !all.iter().any(|rule| &rule.name == name) {
        return Err(format!("no alert rule '{name}'; see 'crane alert list'"));
    }
    match action {
        "remove" => all.retain(|rule| &rule.name != name),
        _ => all
            .iter_mut()
            .filter(|rule| &rule.name == name)
            .for_each(|rule| rule.enabled = action == "enable"),
    }
    rules::save(&workspace, &all)?;
    println!(
        "Alert rule '{name}' {}.",
        match action {
            "remove" => "removed",
            "enable" => "enabled",
            _ => "disabled",
        }
    );
    Ok(())
}

/** Evaluate every rule now
 * Input
    - json: bool - print JSON
 * Output
    - Result<(), String>
*/
fn check(json: bool) -> Result<(), String> {
    require_human("crane alert check")?;
    let workspace = Workspace::locate()?;
    let stores = Stores::open(&workspace.trust()?.runtime());
    let records = rules::evaluate(&workspace, &stores, "COMMAND")?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&records).unwrap_or_default()
        );
        return Ok(());
    }
    if records.is_empty() {
        println!("No enabled alert rules (or another evaluation is running).");
    }
    for record in records {
        println!(
            "{:<24} {:<7} value {} (threshold {} {}) over {} events{}",
            record["rule"].as_str().unwrap_or_default(),
            record["state"].as_str().unwrap_or_default(),
            record["value"]
                .as_f64()
                .map_or_else(|| "unknown".into(), |value| value.to_string()),
            record["op"].as_str().unwrap_or_default(),
            record["threshold"],
            record["events"],
            record["transition"]
                .as_str()
                .map(|transition| format!(
                    " → {transition}, notified: {}",
                    deliveries(&record["deliveries"])
                ))
                .unwrap_or_default()
        );
    }
    Ok(())
}

/** Describe delivery records in one line
 * Input
    - deliveries: &Value - list of {channel, result}
 * Output
    - String
*/
fn deliveries(deliveries: &Value) -> String {
    deliveries
        .as_array()
        .map(|list| {
            list.iter()
                .map(|delivery| {
                    format!(
                        "{} {}",
                        delivery["channel"].as_str().unwrap_or_default(),
                        delivery["result"].as_str().unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join("; ")
        })
        .unwrap_or_else(|| "none".into())
}

/** Send a test notification of a rule to its channels
 * Input
    - name: Option<&String> - rule
 * Output
    - Result<(), String>
*/
fn test(name: Option<&String>) -> Result<(), String> {
    require_human("crane alert test")?;
    let name = name.ok_or(USAGE)?;
    let workspace = Workspace::locate()?;
    let rule = rules::load(&workspace)
        .into_iter()
        .find(|rule| &rule.name == name)
        .ok_or_else(|| format!("no alert rule '{name}'"))?;
    let stores = Stores::open(&workspace.trust()?.runtime());
    let record = rules::notify(
        &workspace,
        &stores,
        &rule,
        rule.threshold,
        "TEST",
        now_millis(),
    )?;
    println!(
        "Test notification of '{name}': {}",
        deliveries(&record["deliveries"])
    );
    Ok(())
}

/** Show the newest rule notifications and their deliveries
 * Input
    - json: bool - print JSON
 * Output
    - Result<(), String>
*/
fn log(json: bool) -> Result<(), String> {
    let workspace = Workspace::locate()?;
    let stores = Stores::open(&workspace.trust()?.runtime());
    let mut alerts = read_lines(&stores.directory.join("alerts.jsonl"))?
        .into_iter()
        .filter(|alert| alert["kind"] == "rule")
        .collect::<Vec<_>>();
    alerts.reverse();
    alerts.truncate(50);
    if json {
        println!("{}", json!(alerts));
        return Ok(());
    }
    if alerts.is_empty() {
        println!("No rule has notified yet.");
    }
    for alert in alerts {
        println!(
            "{}  {:<8} {}  value {}  {}",
            rules::stamp(alert["at"].as_u64().unwrap_or(0) * 1000),
            alert["state"].as_str().unwrap_or_default(),
            alert["title"].as_str().unwrap_or_default(),
            alert["value"],
            deliveries(&alert["deliveries"])
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /** Rules parse from words, with every setting, several filters, and clear errors
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn rules_parse_from_words() {
        let words = |text: &str| {
            text.split_whitespace()
                .map(String::from)
                .collect::<Vec<_>>()
        };
        let rule = parse(&words("too-many-denials metric denied at-least 5 window 2h channels slack,whatsapp severity critical cooldown 45 where severity is HIGH,CRITICAL where tool exists where decision.decision_reasons contains preserved quiet-resolve"), "me".into(), 1).unwrap();
        assert_eq!(rule.op, "at_least");
        assert_eq!(rule.threshold, 5.0);
        assert_eq!(rule.window_minutes, 120);
        assert_eq!(rule.channels, ["slack", "whatsapp"]);
        assert_eq!(rule.severity, "CRITICAL");
        assert_eq!(rule.cooldown_minutes, 45);
        assert!(!rule.notify_resolved);
        assert_eq!(rule.filter.len(), 3);
        assert_eq!(
            rule.filter[0].path, "severity.severity_status",
            "facet shortcuts become paths"
        );
        assert_eq!(rule.filter[0].values, ["HIGH", "CRITICAL"]);
        assert_eq!(rule.filter[1].op, Op::Exists);
        assert!(rule.filter[1].values.is_empty());
        assert_eq!(
            parse(&words("r metric denied above 1"), "me".into(), 1)
                .unwrap()
                .channels,
            ["slack"],
            "slack by default"
        );
        assert!(
            parse(&words("r metric denied"), "me".into(), 1).is_err(),
            "a comparison is required"
        );
        assert!(parse(&words("r metric denied above five"), "me".into(), 1).is_err());
        assert!(parse(&words("r metric denied above 1 window 3w"), "me".into(), 1).is_err());
        assert!(parse(
            &words("r metric denied above 1 channels email"),
            "me".into(),
            1
        )
        .is_err());
        assert!(parse(
            &words("r metric denied above 1 where flow sideways F05"),
            "me".into(),
            1
        )
        .is_err());
        // The added operators and exclusion
        let rule = parse(&words("r metric events above 1 where event like TOOL_CALL_* where eval.latency_ms between 100,500 except tool is READ where flow suffix 5"), "me".into(), 1).unwrap();
        assert_eq!(rule.filter[0].op, Op::Like);
        assert_eq!(rule.filter[1].op, Op::Between);
        assert_eq!(rule.filter[1].values, ["100", "500"]);
        assert!(rule.filter[2].exclude && rule.filter[2].path == "action.action_tool");
        assert!(!rule.filter[3].exclude && rule.filter[3].op == Op::Suffix);
        assert!(rule.describe().contains("not action.action_tool"));
        assert!(
            parse(
                &words("r metric events above 1 where eval.latency_ms between 100"),
                "me".into(),
                1
            )
            .is_err(),
            "between needs two numbers"
        );
    }
}
