// Tests of the flow knowledge base: deterministic tracing and its drafts, appending a flow to
// governance.yaml without touching the rest, model routing and its rule, the agent channel's
// parsing, and the watcher limits.

use std::collections::BTreeSet;

use serde_json::json;

use super::agent::{slug, words};
use super::context::{guidance, model_findings};
use super::model::{route, tier_of, FlowBook, FlowKnowledge, ModelEntry, Priority, Tier};
use super::store::{append_flow, NewFlow, NewPack};
use super::trace::{add_revision, check_starts, diff, rewind, trace_index};
use super::watch::{may_start, Parent, Watcher};
use crate::governance::flows::Index;

/** A small repository: submit calls charge and audit; charge calls send
 * Input
    - with_refund: bool - charge also calls refund (a changed codebase)
 * Output
    - Index
*/
fn index(with_refund: bool) -> Index {
    let charge = if with_refund {
        "def charge(x):\n    send(x)\n    return refund(x)\n\ndef refund(x):\n    return x\n"
    } else {
        "def charge(x):\n    return send(x)\n"
    };
    Index::build([
        (
            "api/pay.py".to_string(),
            format!("def submit(x):\n    audit(x)\n    return charge(x)\n\n{charge}"),
        ),
        (
            "api/net.py".to_string(),
            "def send(x):\n    return x\n".to_string(),
        ),
        (
            "api/log.py".to_string(),
            "def audit(x):\n    return None\n".to_string(),
        ),
    ])
}

/** Tracing is deterministic, its digest names what was found (not when), a change of the code
 * shows as a difference, and drafts keep revisions: retrace adds one, rewind moves back without
 * deleting anything
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn traces_are_deterministic_and_drafts_keep_revisions() {
    let starts = vec!["submit".to_string()];
    let first = trace_index(&index(false), &starts, 8);
    let again = trace_index(&index(false), &starts, 8);
    assert_eq!(first, again);
    assert_eq!(first.digest(), again.digest());
    assert_eq!(first.status, "RESOLVED");
    assert_eq!(first.nodes.len(), 4, "{:?}", first.nodes);
    assert_eq!(first.files, vec!["api/log.py", "api/net.py", "api/pay.py"]);
    // Depth bounds the walk
    assert_eq!(trace_index(&index(false), &starts, 1).nodes.len(), 3);
    let changed = trace_index(&index(true), &starts, 8);
    assert_ne!(changed.digest(), first.digest());
    let changes = diff(&first, &changed);
    assert_eq!(changes.added_nodes, vec!["api/pay.py:refund"]);
    assert!(changes.removed_nodes.is_empty());
    assert!(diff(&first, &again).is_empty());

    let draft = add_revision(
        None,
        "checkout",
        "Pay for an order",
        starts.clone(),
        8,
        first.clone(),
        "ana",
        "cli",
        "traced",
        1,
    );
    assert_eq!((draft.current, draft.revisions.len()), (1, 1));
    let mut draft = add_revision(
        Some(draft),
        "checkout",
        "",
        starts.clone(),
        8,
        changed,
        "ana",
        "cli",
        "code changed",
        2,
    );
    assert_eq!((draft.current, draft.revisions.len()), (2, 2));
    assert_eq!(
        draft.description, "Pay for an order",
        "an empty description keeps the old one"
    );
    rewind(&mut draft, 1, 3).unwrap();
    assert_eq!(draft.current, 1);
    assert_eq!(draft.revisions.len(), 2, "rewinding deletes nothing");
    assert_eq!(draft.current().unwrap().digest, first.digest());
    assert!(rewind(&mut draft, 9, 4).is_err());
    // A third revision continues the numbering
    let draft = add_revision(
        Some(draft),
        "checkout",
        "",
        starts,
        8,
        first,
        "ana",
        "cli",
        "again",
        5,
    );
    assert_eq!(draft.current, 3);

    // Unresolved starting points are reported, never guessed
    let missing = trace_index(&index(false), &["nothing".to_string()], 8);
    assert_eq!(missing.status, "UNRESOLVED");
    for bad in [vec![], vec!["../x".to_string()], vec!["a b".to_string()]] {
        assert!(check_starts(&bad, 8).is_err(), "{bad:?}");
    }
    assert!(check_starts(&["submit".to_string()], 0).is_err());
    assert_eq!(
        check_starts(&["a.py:f, g".to_string()], 8).unwrap(),
        vec!["a.py:f", "g"]
    );
}

/** Approving a flow appends exactly one entry to governance.yaml, in the file's own list style,
 * keeping comments, other keys, and line endings; anything else is refused
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn approved_flows_are_appended_without_touching_the_rest() {
    let entry = vec!["api/pay.py:submit".to_string()];
    let flow = |id: &str, description: &str, status: &str| NewFlow {
        id: id.into(),
        description: description.into(),
        entry_points: entry.clone(),
        max_depth: 8,
        status: status.into(),
        pack: format!("{id}-flow"),
    };
    let pack = |id: &str| NewPack {
        id: format!("{id}-flow"),
        description: "guidance".into(),
        content: vec!["Starts at submit.".into()],
    };
    let file = "version: 1\n# the payment zone\nzones:\n  - id: payments\n    description: Payments\n    selectors:\n      paths: [\"api/**\"]\nflows:\n  - id: refund\n    entry_points: [refund]\ncontext_packs: []\n";
    let added = append_flow(
        file,
        &flow("checkout", "Pay \"now\"", "active"),
        &pack("checkout"),
    )
    .unwrap();
    assert!(added.contains("# the payment zone"), "comments are kept");
    assert!(added.contains("  - id: \"checkout\""), "{added}");
    assert!(added.contains("\"Pay \\\"now\\\"\""), "{added}");
    assert!(
        added.contains("context_packs:\n  - id: \"checkout-flow\""),
        "the inline empty pack list becomes a block list: {added}"
    );
    let parsed: crate::governance::zones::Governance = serde_yaml::from_str(&added).unwrap();
    assert_eq!(parsed.flows.len(), 2);
    assert_eq!(parsed.flows[1].entry_points, entry);
    assert_eq!(parsed.flows[1].max_depth, Some(8));
    assert_eq!(parsed.flows[1].context_packs, vec!["checkout-flow"]);
    assert_eq!(parsed.context_packs[0].content, vec!["Starts at submit."]);
    // An inline empty list, a missing list, an empty file, items at column 0, and CRLF
    for text in [
        "version: 1\nflows: []\n",
        "version: 1\nzones: []\n",
        "",
        "version: 1\nflows:\n- id: refund\n  entry_points: [refund]\n",
        "version: 1\r\nflows:\r\n  - id: refund\r\n    entry_points: [refund]\r\n",
    ] {
        let added = append_flow(text, &flow("checkout", "", "proposed"), &pack("checkout"))
            .unwrap_or_else(|error| panic!("{text:?}: {error}"));
        let parsed: crate::governance::zones::Governance = serde_yaml::from_str(&added).unwrap();
        assert!(
            parsed.flows.iter().any(|flow| flow.id == "checkout"),
            "{added}"
        );
        if text.contains("\r\n") {
            assert!(
                !added.replace("\r\n", "").contains('\n'),
                "line endings are kept"
            );
        }
    }
    assert!(
        append_flow(file, &flow("refund", "", "active"), &pack("refund"))
            .unwrap_err()
            .contains("already")
    );
    assert!(append_flow(
        "version: 1\nflows: [{id: a}]\n",
        &flow("checkout", "", "active"),
        &pack("checkout")
    )
    .is_err());
}

/** A priority routes to the first available model of its tiers (the asking provider's first), the
 * routing rule holds changes from models outside those tiers only when the flow enforces it, and
 * the guidance an agent receives names the routing and the required tests
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn priorities_route_models_and_the_rule_only_tightens() {
    let mut book = FlowBook::default();
    for (provider, model, tier, available) in [
        ("claude", "claude-opus-5-5", Tier::Frontier, true),
        ("codex", "gpt-frontier", Tier::Frontier, true),
        ("claude", "claude-sonnet-5-5", Tier::Balanced, true),
        ("claude", "claude-haiku-5-5", Tier::Fast, false),
    ] {
        book.models.push(ModelEntry {
            provider: provider.into(),
            model: model.into(),
            tier,
            available,
        });
    }
    assert_eq!(
        route(&book, Priority::P0, None).1.unwrap().model,
        "claude-opus-5-5"
    );
    assert_eq!(
        route(&book, Priority::P0, Some("codex")).1.unwrap().model,
        "gpt-frontier"
    );
    // The fast model is unavailable, so P3 falls back to balanced
    assert_eq!(
        route(&book, Priority::P3, None).1.unwrap().model,
        "claude-sonnet-5-5"
    );
    book.routing.insert(Priority::P2, vec![Tier::Fast]);
    assert!(
        route(&book, Priority::P2, None).1.is_none(),
        "an override is followed exactly"
    );
    assert_eq!(tier_of(&book, "CLAUDE-SONNET-5-5"), Some(Tier::Balanced));

    let flows = BTreeSet::from(["checkout".to_string()]);
    book.flows.insert(
        "checkout".into(),
        FlowKnowledge {
            priority: Some(Priority::P0),
            ..FlowKnowledge::default()
        },
    );
    assert!(
        model_findings(&book, &flows, Some("claude-sonnet-5-5")).is_empty(),
        "guidance only until enforced"
    );
    book.flows.get_mut("checkout").unwrap().enforce_model = true;
    assert!(model_findings(&book, &flows, Some("claude-opus-5-5")).is_empty());
    assert_eq!(
        model_findings(&book, &flows, Some("claude-sonnet-5-5")).len(),
        1
    );
    assert_eq!(
        model_findings(&book, &flows, Some("unknown-model")).len(),
        1
    );
    assert_eq!(
        model_findings(&book, &flows, None).len(),
        1,
        "an unreported model is outside"
    );
    let text = guidance(&book, &flows, Some("claude-sonnet-5-5")).unwrap();
    assert!(
        text.contains("Priority P0") && text.contains("not one of these"),
        "{text}"
    );
    assert!(guidance(&book, &BTreeSet::from(["other".to_string()]), None).is_none());
}

/** The agent channel is one command: quoted text stays whole, shell operators make it malformed,
 * and anything else is not a request
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn requests_are_one_command() {
    assert!(words("ls -la").is_none());
    assert!(words("echo crane-request").is_none());
    assert_eq!(
        words("crane-request test --flow checkout --name 'charges once' --description \"says \\\"once\\\"\"").unwrap().unwrap(),
        vec!["crane-request", "test", "--flow", "checkout", "--name", "charges once", "--description", "says \"once\""]
    );
    assert_eq!(
        words("/usr/bin/crane-request help").unwrap().unwrap()[1],
        "help"
    );
    for bad in [
        "crane-request flows; rm -rf /",
        "crane-request flows | cat",
        "crane-request flows && echo",
        "crane-request flow $(whoami)",
        "crane-request flow `id`",
        "crane-request flows > out",
        "crane-request flow 'open",
    ] {
        assert!(words(bad).unwrap().is_err(), "{bad}");
    }
    assert_eq!(
        words("crane-request flow 'a;b'").unwrap().unwrap()[2],
        "a;b",
        "operators inside quotes are text"
    );
    assert_eq!(
        slug("api/pay.py:CheckoutService.submit"),
        "checkout-service-submit"
    );
    assert_eq!(slug("9lives"), "flow-9lives");
}

/** Watchers are capped per flow and per agent session by the signed limits; a person is counted
 * only against the flow's limit, and a flow can only lower the repository's limit
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn watchers_are_capped_by_signed_limits() {
    let mut book = FlowBook::default();
    book.limits.max_watchers_per_session = 1;
    book.limits.max_watchers_per_flow = 2;
    let watcher = |id: &str, flow: &str, session: &str| Watcher {
        watcher_id: id.into(),
        flow: flow.into(),
        purpose: "watch".into(),
        parent: Parent {
            session: session.into(),
            ..Parent::default()
        },
        created_by: "agent".into(),
        created_at: 1,
        active: true,
        stopped_by: None,
        inherited: Default::default(),
        checks: vec!["denials".into()],
        firing: Vec::new(),
        last_run: 0,
    };
    let mut existing = Vec::new();
    assert!(may_start(&book, &existing, "checkout", Some("s1")).is_ok());
    existing.push(watcher("w1", "checkout", "s1"));
    assert!(may_start(&book, &existing, "checkout", Some("s1"))
        .unwrap_err()
        .contains("this session"));
    assert!(may_start(&book, &existing, "checkout", Some("s2")).is_ok());
    existing.push(watcher("w2", "checkout", "s2"));
    assert!(
        may_start(&book, &existing, "checkout", None)
            .unwrap_err()
            .contains("limit"),
        "the flow is full, for people too"
    );
    existing[0].active = false;
    assert!(
        may_start(&book, &existing, "checkout", Some("s1")).is_ok(),
        "stopped watchers do not count"
    );
    book.flows.insert(
        "checkout".into(),
        FlowKnowledge {
            max_watchers: Some(9),
            ..FlowKnowledge::default()
        },
    );
    assert_eq!(
        book.watcher_limit("checkout"),
        2,
        "a flow cannot raise the repository's limit"
    );
    book.flows.get_mut("checkout").unwrap().max_watchers = Some(0);
    assert!(may_start(&book, &[], "checkout", None).is_err());
    assert_eq!(json!(book.limits)["max_watchers_per_session"], 1);
}
