// Work Capture through the crane binary: the deterministic demonstration, and investigations over
// a real repository's live sources (Git, DataSea's event store, Landscape, Crane policies).

mod common;

use std::fs;

use common::{text, Fixture};
use serde_json::{json, Value};

/** The stored DataSea events of the fixture
 * Input
    - fixture: &Fixture - repository
 * Output
    - Vec<Value>
*/
fn events(fixture: &Fixture) -> Vec<Value> {
    let repos = fixture.home.join("repos");
    let directory = fs::read_dir(&repos)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
        .join("runtime");
    fs::read_to_string(directory.join("events.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/** The demonstration produces a deterministic, bounded, incomplete pack that reports the missing
 * test report and unavailable metrics instead of concluding, without leaking the logged secret
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn demo_produces_an_honest_evidence_pack() {
    let fixture = Fixture::new();
    let (ok, report) = fixture.json(&["capture", "demo", "--json"]);
    assert!(ok);
    let pack = &report["pack"];
    assert_eq!(pack["status"], json!("incomplete"));
    let missing = pack["missing_evidence"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            (
                item["evidence_type"].as_str().unwrap().to_string(),
                item["state"].as_str().unwrap().to_string(),
            )
        })
        .collect::<Vec<_>>();
    assert!(
        missing.contains(&("test_report".into(), "empty".into())),
        "{missing:?}"
    );
    assert!(
        missing.contains(&("metric".into(), "unavailable".into())),
        "{missing:?}"
    );
    let tests = pack["assertions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|assertion| assertion["claim"]["predicate"] == json!("tests_passed"))
        .unwrap();
    assert_eq!(
        (tests["claim"]["object"].as_str(), tests["status"].as_str()),
        (Some("unknown"), Some("unresolved"))
    );
    assert_eq!(report["replay"]["fidelity"], json!("exact"));
    assert_eq!(report["episode"]["outcome"], json!("failed"));
    assert!(pack["manifest"]["entries"].as_array().unwrap().len() > 10);
    let (_, again) = fixture.json(&["capture", "demo", "--json"]);
    assert_eq!(again["pack"]["pack_id"], pack["pack_id"]);
    let output = fixture.ok(&["capture", "demo"]);
    assert!(
        output.contains("Missing evidence:") && output.contains("TestReport (Empty)"),
        "{output}"
    );
    assert!(
        !output.contains("Sup3rS3cretValue") && !report.to_string().contains("Sup3rS3cretValue")
    );
}

/** Investigating a commit of a real repository collects Git, Landscape, Crane, and DataSea
 * evidence, keeps the pack in the trust directory, and an agent cannot run it
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn investigates_a_local_change() {
    let fixture = Fixture::ready();
    fixture.write(".crane/governance.yaml", "version: 1\nzones:\n  - id: payments\n    version: 2\n    selectors:\n      directories: [app]\n");
    fixture.ok(&["governance", "sign"]);
    fixture.commit("govern payments");
    fixture.write(
        "app/refunds.py",
        "def refund(amount):\n    return -amount\n",
    );
    fixture.commit("add refunds");
    let read = json!({"session_id": "s-capture", "tool_use_id": "t1", "cwd": fixture.work.to_string_lossy(), "tool_name": "Read", "tool_input": {"file_path": fixture.work.join("app/refunds.py").to_string_lossy()}});
    let decided = fixture.hook("claude", "pre-tool-use", &read);
    assert!(decided.status.success(), "{}", text(&decided));
    let (ok, sources) = fixture.json(&["capture", "sources", "--json"]);
    assert!(ok);
    let available = sources
        .as_array()
        .unwrap()
        .iter()
        .filter(|source| source["available"] == json!(true))
        .map(|source| source["resource_id"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(available, vec!["crane", "datasea", "git", "landscape"]);
    let task = events(&fixture)
        .iter()
        .find_map(|event| event["scope"]["foxx_task_id"].as_str().map(str::to_string))
        .expect("the hook event carries its task");
    let (ok, result) = fixture.json(&["capture", "investigate", "task", &task, "--json"]);
    assert!(ok, "{result}");
    let pack = &result["pack"];
    let types = pack["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .map(|evidence| evidence["evidence_type"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    for wanted in ["code_change", "graph_state", "validation"] {
        assert!(
            types.iter().any(|kind| kind == wanted),
            "{wanted} missing from {types:?}"
        );
    }
    assert!(types.iter().any(|kind| kind == "tool_call"), "{types:?}");
    assert_eq!(
        pack["status"],
        json!("sufficient"),
        "{}",
        pack["missing_evidence"]
    );
    let code = pack["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .find(|evidence| evidence["evidence_type"] == json!("code_change"))
        .unwrap();
    assert!(code["excerpt"].to_string().contains("app/refunds.py"));
    assert!(code["artifact"]["storage_reference"]
        .as_str()
        .unwrap()
        .starts_with("blob:"));
    let zone = pack["entities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entity| entity["kind"] == json!("zone") && entity["version"] == json!("v2"));
    assert!(zone, "the Landscape zone is referenced with its version");
    assert!(pack["context_view"]["relations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|relation| relation["relation_type"] == json!("BELONGS_TO_ZONE")));
    assert_eq!(
        result["audit"]["recipe"]["recipe_id"],
        json!("local-change")
    );
    let runtime = fs::read_dir(fixture.home.join("repos"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
        .join("runtime")
        .join("capture");
    assert!(fs::read_to_string(runtime.join("packs.jsonl"))
        .unwrap()
        .contains(pack["pack_id"].as_str().unwrap()));
    assert!(fs::read_to_string(runtime.join("audit.jsonl"))
        .unwrap()
        .contains(result["audit"]["query_id"].as_str().unwrap()));
    let refused = fixture.as_agent(&["capture", "investigate"]);
    assert!(
        !refused.status.success() && text(&refused).contains("cannot be executed by an AI agent")
    );
    let unknown = fixture.fails(&["capture", "investigate", "commit", "no-such-commit"]);
    assert!(unknown.contains("is not a commit"), "{unknown}");
    // A repeated investigation without a task (a new episode) still packs the evidence it
    // re-observed as duplicates of the first run
    let (_, first) = fixture.json(&["capture", "investigate", "--json"]);
    let (_, second) = fixture.json(&["capture", "investigate", "--json"]);
    assert_ne!(first["pack"]["episode_id"], second["pack"]["episode_id"]);
    assert!(second["audit"]["metrics"]["duplicates"].as_u64().unwrap() > 0);
    assert_eq!(
        second["pack"]["evidence"].as_array().unwrap().len(),
        first["pack"]["evidence"].as_array().unwrap().len()
    );
}

/** The capture commands cover an investigation's lifecycle: replay it, verify its pack, propose
 * context, refuse reviewing one's own proposal, and record the episode's outcome only with
 * evidence of the episode
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn capture_commands_cover_the_lifecycle() {
    let fixture = Fixture::ready();
    fixture.write(
        "app/refunds.py",
        "def refund(amount):\n    return -amount\n",
    );
    fixture.commit("add refunds");
    let (ok, result) = fixture.json(&["capture", "investigate", "--json"]);
    assert!(ok, "{result}");
    let query = result["audit"]["query_id"].as_str().unwrap().to_string();
    let pack = result["pack"]["pack_id"].as_str().unwrap().to_string();
    let episode = result["pack"]["episode_id"].as_str().unwrap().to_string();
    let evidence = result["pack"]["evidence"][0]["evidence_id"]
        .as_str()
        .unwrap()
        .to_string();

    let (ok, replayed) = fixture.json(&["capture", "replay", &query, "--json"]);
    assert!(ok, "{replayed}");
    assert_eq!(replayed["audit"]["replay"]["original_query"], json!(query));
    assert_eq!(replayed["pack"]["episode_id"], json!(episode));

    let verified = fixture.ok(&["capture", "verify", &pack]);
    assert!(verified.contains("matches its manifest"), "{verified}");
    let unknown = fixture.fails(&["capture", "verify", "pck_missing"]);
    assert!(unknown.contains("no Evidence Pack"), "{unknown}");

    let proposal = json!({
        "target_ref": {"kind": "work_episode", "id": episode, "version": null},
        "episode_id": episode,
        "contribution_type": "observation",
        "namespace": "observed",
        "content": {"observation": "the refund returns a negative amount"},
        "evidence_refs": [evidence],
        "locality": "task_local",
        "valid_until": null
    });
    fixture.write("proposal.json", &proposal.to_string());
    let proposal_path = fixture.work.join("proposal.json");
    let (ok, contributed) = fixture.json(&[
        "capture",
        "contribute",
        proposal_path.to_str().unwrap(),
        "--json",
    ]);
    assert!(ok, "{contributed}");
    assert_eq!(contributed["status"], json!("proposed"));
    let id = contributed["contribution_id"].as_str().unwrap().to_string();
    let own = fixture.fails(&["capture", "review", &id, "accept", "reason", "looks right"]);
    assert!(own.contains("cannot review their own"), "{own}");

    let fabricated = fixture.fails(&[
        "capture",
        "outcome",
        &episode,
        "succeeded",
        "evidence",
        "evd_made_up",
    ]);
    assert!(fabricated.contains("does not exist"), "{fabricated}");
    let recorded = fixture.ok(&[
        "capture",
        "outcome",
        &episode,
        "partially_verified",
        "evidence",
        &evidence,
    ]);
    assert!(recorded.contains("PartiallyVerified"), "{recorded}");

    let agent = fixture.as_agent(&["capture", "contribute", proposal_path.to_str().unwrap()]);
    assert!(
        !agent.status.success(),
        "an agent cannot act as the operator"
    );
}

/** Questions and relation confirmations through the crane binary: a question is asked, answered
 * by a kept pack, and then cannot move again; the owner confirms a relation of the pack's view
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn questions_and_confirmations_through_the_cli() {
    let fixture = Fixture::ready();
    fixture.write(
        "app/refunds.py",
        "def refund(amount):\n    return -amount\n",
    );
    fixture.commit("add refunds");
    let (ok, result) = fixture.json(&["capture", "investigate", "--json"]);
    assert!(ok, "{result}");
    let pack = result["pack"]["pack_id"].as_str().unwrap().to_string();
    let episode = result["pack"]["episode_id"].as_str().unwrap().to_string();
    let relation = result["pack"]["context_view"]["relations"][0]["relation_id"]
        .as_str()
        .expect("the view has a relation")
        .to_string();

    let confirmed = fixture.ok(&["capture", "confirm", &relation]);
    assert!(confirmed.contains("confirmed as"), "{confirmed}");

    let question = json!({
        "question_id": "q-tests",
        "text": "Did the refund tests run?",
        "target": {"kind": "work_episode", "id": episode, "version": null},
        "reason": "no test report",
        "relevant_evidence": [],
        "required_evidence": ["test_report"],
        "permitted_resources": ["git"],
        "recipe": null
    });
    fixture.write("question.json", &question.to_string());
    let path = fixture.work.join("question.json");
    let asked = fixture.ok(&["capture", "ask", path.to_str().unwrap()]);
    assert!(asked.contains("Asked q-tests"), "{asked}");
    let unanswered = fixture.fails(&["capture", "question", "q-tests", "answer", "pck_missing"]);
    assert!(unanswered.contains("kept Evidence Pack"), "{unanswered}");
    let answered = fixture.ok(&["capture", "question", "q-tests", "answer", &pack]);
    assert!(answered.contains("Answered"), "{answered}");
    let moved = fixture.fails(&["capture", "question", "q-tests", "block"]);
    assert!(moved.contains("cannot become"), "{moved}");
}

/** The learning loop through the crane binary: a question class no recipe serves is explored over
 * the live sources, the person judges the explorations, Insight reports the question and proposes
 * a recipe, the owner accepts it, and the class is then served by the learned recipe; an agent can
 * run none of it
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn explorations_become_a_learned_recipe_through_the_cli() {
    let fixture = Fixture::ready();
    fixture.write("app/notes.py", "def notes():\n    return []\n");
    fixture.commit("add notes");
    let mut packs = Vec::new();
    for _ in 0..3 {
        let (ok, result) =
            fixture.json(&["capture", "investigate", "class", "release-notes", "--json"]);
        assert!(ok, "{result}");
        assert_eq!(result["pack"]["status"], json!("exploratory"));
        assert!(result["audit"]["recipe"]["recipe_id"]
            .as_str()
            .unwrap()
            .starts_with("explore-"));
        packs.push(result["pack"]["pack_id"].as_str().unwrap().to_string());
    }
    for pack in &packs[..2] {
        let judged = fixture.ok(&["capture", "feedback", pack, "sufficient", "yes"]);
        assert!(judged.contains("sufficient Yes"), "{judged}");
    }
    let wrong = fixture.fails(&["capture", "feedback", &packs[2], "sufficient", "maybe"]);
    assert!(wrong.contains("usage"), "{wrong}");

    let (ok, report) = fixture.json(&["insight", "report", "--json"]);
    assert!(ok, "{report}");
    let question = report["patterns"]
        .as_array()
        .unwrap()
        .iter()
        .find(|pattern| pattern["kind"] == json!("question"))
        .cloned()
        .expect("the unserved question is a pattern");
    assert_eq!(question["destination"]["to"], json!("recipe"));
    let listed = fixture.ok(&["insight", "reports"]);
    assert!(
        listed.contains(report["report_id"].as_str().unwrap()),
        "{listed}"
    );

    let proposed = fixture.ok(&[
        "insight",
        "propose",
        question["pattern_id"].as_str().unwrap(),
    ]);
    assert!(proposed.contains("Proposed recipe"), "{proposed}");
    let (ok, recipes) = fixture.json(&["insight", "recipes", "--json"]);
    assert!(ok, "{recipes}");
    let proposal = recipes["proposals"][0].clone();
    assert_eq!(proposal["status"], json!("proposed"));
    assert_eq!(
        proposal["recipe"]["recipe_id"],
        json!("learned-release-notes")
    );
    let id = proposal["proposal_id"].as_str().unwrap().to_string();
    let agent = fixture.as_agent(&["insight", "review", &id, "accept", "reason", "fine"]);
    assert!(!agent.status.success(), "an agent cannot review a recipe");
    let accepted = fixture.ok(&[
        "insight",
        "review",
        &id,
        "accept",
        "reason",
        "what we check",
    ]);
    assert!(accepted.contains("Accepted"), "{accepted}");

    let (ok, learned) =
        fixture.json(&["capture", "investigate", "class", "release-notes", "--json"]);
    assert!(ok, "{learned}");
    assert_eq!(
        learned["pack"]["retrieval"]["recipe_id"],
        json!("learned-release-notes")
    );
    assert_ne!(learned["pack"]["status"], json!("exploratory"));
}
