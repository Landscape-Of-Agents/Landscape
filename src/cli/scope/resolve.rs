// crane scope resolve: decide a proposed action as an agent's hook would (the same guard), resolve
// its target to what it really is, list what Crane cannot enforce for it, and record the decision
// with the rule and resource versions it was made under.

use super::{name, print_json, print_rules};
use crate::cli::args::Args;
use crate::platform::{actor, now_millis};
use crate::scope::decision::{DecisionLog, ScopeDecision};
use crate::scope::resource::{Location, Tracking};
use crate::scope::{Access, Request};

/** Usage of the command */
const USAGE: &str = "crane scope resolve read|write|delete|execute|network TARGET [lines A-B] [mode observe|assisted|delegated|autonomous] [--json]
  (quote a shell command as one TARGET)";

/** Parse "A-B" or "N" as an inclusive line range
 * Input
    - value: &str - text
 * Output
    - Result<(usize, usize), String>
*/
fn parse_lines(value: &str) -> Result<(usize, usize), String> {
    let number = |text: &str| {
        text.trim()
            .parse::<usize>()
            .ok()
            .filter(|line| *line > 0)
            .ok_or_else(|| {
                format!("lines must be N or A-B with positive line numbers, got '{value}'")
            })
    };
    match value.split_once('-') {
        Some((first, last)) => Ok((number(first)?, number(last)?)),
        None => number(value).map(|line| (line, line)),
    }
}

/** Describe where a resolved target is, in words
 * Input
    - resource: &crate::scope::resource::Resource - resolved target
 * Output
    - String
*/
fn describe(resource: &crate::scope::resource::Resource) -> String {
    let path = resource
        .path
        .clone()
        .or_else(|| resource.absolute.clone())
        .unwrap_or_else(|| resource.requested.clone());
    let place = match resource.location {
        Location::Repository => {
            let tracking = match resource.tracking {
                Tracking::Tracked => "tracked",
                Tracking::Untracked => "untracked, not ignored",
                Tracking::Ignored => "ignored by Git",
                Tracking::Missing => "does not exist yet",
                Tracking::NestedRepository => "in a nested repository",
                Tracking::NotApplicable => "",
            };
            format!("repository file {path} ({tracking})")
        }
        Location::CraneMetadata => format!("Crane metadata {path}"),
        Location::TrustDirectory => format!("the Crane trust directory ({path})"),
        Location::HookSettings => format!("agent hook settings {path}"),
        Location::OtherWorktree => format!("another worktree of this repository ({path})"),
        Location::OutsideRepository => format!("outside the repository ({path})"),
        Location::Url => format!("network resource {}", resource.requested),
        Location::Command => format!("shell command: {}", resource.requested),
    };
    if resource.via_symlink {
        format!("{place}, reached through a symlink")
    } else {
        place
    }
}

/** Run the command
 * Input
    - args: &[String] - arguments after "resolve"
 * Output
    - Result<(), String>
*/
pub(super) fn resolve(args: &[String]) -> Result<(), String> {
    let args = Args::parse(
        args,
        &["lines", "mode", "resource", "format", "request-id"],
        &["json"],
    )?;
    if let (Some(resource), true) = (args.value("resource"), args.positional.is_empty()) {
        return super::skill::resolve(&args, &resource);
    }
    args.expect_positional(2, USAGE)?;
    let access = Access::parse(&args.positional[0]).ok_or_else(|| format!("usage: {USAGE}"))?;
    let lines = args
        .value("lines")
        .as_deref()
        .map(parse_lines)
        .transpose()?;
    if lines.is_some() && access != Access::Write {
        return Err("lines apply only to write".into());
    }
    let mode = args.value("mode").unwrap_or_else(|| "delegated".into());
    if !["observe", "assisted", "delegated", "autonomous"].contains(&mode.as_str()) {
        return Err(format!(
            "mode must be observe, assisted, delegated, or autonomous, got '{mode}'"
        ));
    }
    let request = Request {
        access,
        target: args.positional[1].clone(),
        lines,
        mode,
    };
    let boundary = super::boundary()?;
    let outcome = boundary.decide(&request)?;
    let recorded = DecisionLog::open(&boundary.runtime).append(ScopeDecision::new(
        &boundary,
        &request,
        &outcome,
        &actor(),
        now_millis(),
    ))?;
    if crate::cli::protocol::wants_json(&args)? {
        return print_json(&recorded);
    }
    let range = lines
        .map(|(first, last)| format!(" lines {first}-{last}"))
        .unwrap_or_default();
    println!(
        "{}  {} {}{range} (mode {})",
        name(&recorded.decision),
        access.name(),
        request.target,
        request.mode
    );
    println!("Resource: {}", describe(&recorded.resource));
    for (label, values) in [
        ("Zones and flows", &recorded.zones),
        ("Selections", &recorded.selections),
        ("Policies", &recorded.policies),
    ] {
        if !values.is_empty() {
            println!("{label}: {}", values.join(", "));
        }
    }
    if recorded.rules.is_empty() && recorded.reasons.is_empty() {
        println!("Why: no rule objects");
    } else {
        println!("Why:");
        print_rules(&recorded.rules, &recorded.reasons);
    }
    if !recorded.gaps.is_empty() {
        println!("Not enforced by Crane for this action:");
        for gap in &recorded.gaps {
            println!("  - {}: {}", gap.code, gap.message);
        }
    }
    if !recorded.integrity_ok {
        println!("Warning: the signed rules fail verification, so every agent change is denied until a person runs 'crane validate'");
    }
    println!(
        "Recorded as {} under generation {} ('crane scope explain {}')",
        recorded.decision_id, recorded.versions.registry_generation, recorded.decision_id
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_lines;

    /** Line ranges parse as N or A-B, positive only
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn line_ranges_parse() {
        assert_eq!(parse_lines("3-9").unwrap(), (3, 9));
        assert_eq!(parse_lines("4").unwrap(), (4, 4));
        assert!(parse_lines("0-2").is_err());
        assert!(parse_lines("a").is_err());
    }
}
