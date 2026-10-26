// crane scope explain: explain a recorded decision (a 'crane scope resolve' answer, or a hook's
// decision by its tool call id): the rules with their Crane operation domain and meaning, the
// governance involved, and whether it still applies to the present rules and file content.

use super::{name, print_json, print_rules};
use crate::cli::args::Args;
use crate::scope::decision::explain as explanation;

/** Run the command
 * Input
    - args: &[String] - arguments after "explain"
 * Output
    - Result<(), String>
*/
pub(super) fn explain(args: &[String]) -> Result<(), String> {
    let args = Args::parse(
        args,
        &["operation", "resource", "skill", "format", "request-id"],
        &["json"],
    )?;
    if let Some(operation) = args.value("operation") {
        let resource = args
            .value("resource")
            .ok_or("'--resource' is required with '--operation'")?;
        return super::skill::explain(&args, &operation, &resource);
    }
    args.expect_positional(1, "crane scope explain DECISION_ID|TOOL_CALL_ID [--json]")?;
    let boundary = super::boundary()?;
    let explained = explanation(&boundary, &args.positional[0])?;
    if crate::cli::protocol::wants_json(&args)? {
        return print_json(&explained);
    }
    println!(
        "{} {}: {} (under generation {})",
        explained.source,
        explained.id,
        name(&explained.decision),
        explained.registry_generation
    );
    let codes = explained
        .rules
        .iter()
        .filter_map(|rule| rule["code"].as_str().map(str::to_string))
        .collect::<Vec<_>>();
    if codes.is_empty() && explained.reasons.is_empty() {
        println!("Why: no rule objected");
    } else {
        println!("Why:");
        print_rules(&codes, &explained.reasons);
    }
    for (label, values) in [
        ("Zones and flows", &explained.zones),
        ("Selections", &explained.selections),
        ("Policies", &explained.policies),
    ] {
        if !values.is_empty() {
            println!("{label}: {}", values.join(", "));
        }
    }
    for gap in &explained.gaps {
        println!("Not enforced by Crane: {}: {}", gap.code, gap.message);
    }
    if explained.changed_since.is_empty() {
        println!("Still applies: the rules and the resource are unchanged");
    } else {
        println!("No longer applies; decide again:");
        for change in &explained.changed_since {
            println!("  - {change}");
        }
    }
    Ok(())
}
