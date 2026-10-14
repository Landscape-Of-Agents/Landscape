// The Signed Selection Registry: anchors, identifiers, digests, signatures, location tracking,
// and detection of marker and registry tampering, rollback, and unauthorized reuse.

mod common;

use std::fs;

use common::{git, Fixture, PAYMENTS};
use serde_json::Value;

/** Find a registry record
 * Input
    - fixture: &Fixture - repository
    - id: &str - selection identifier
 * Output
    - Value
*/
fn record(fixture: &Fixture, id: &str) -> Value {
    fixture.crane_json("registry.json")["selections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["id"] == id)
        .cloned()
        .unwrap_or_else(|| panic!("record {id} exists"))
}

/** protect inserts language-appropriate markers around the range without changing the code,
 * and records a signed record with 128-character SHA-512 digests and a map entry
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn protect_inserts_markers_and_signs_a_record() {
    let fixture = Fixture::ready();
    let id = fixture.protect(&[
        "app/payments.py",
        "start-line",
        "1",
        "end-line",
        "4",
        "name",
        "charge",
    ]);
    assert_eq!(id.len(), 8);
    assert!(id
        .chars()
        .all(|character| character.is_ascii_uppercase() || character.is_ascii_digit()));
    let expected = format!(
        "# @crane:selection:{id}:start\n{}# @crane:selection:{id}:end\n{}",
        &PAYMENTS[..PAYMENTS.find("\n\n\n").unwrap() + 1],
        &PAYMENTS[PAYMENTS.find("\n\n\n").unwrap() + 1..]
    );
    assert_eq!(fixture.read("app/payments.py"), expected);
    let record = record(&fixture, &id);
    for digest in [
        &record["origin"]["content_digest"],
        &record["origin"]["binding_digest"],
        &record["record_digest"],
    ] {
        let digest = digest.as_str().unwrap();
        assert_eq!(digest.len(), 128);
        assert!(digest
            .chars()
            .all(|character| character.is_ascii_hexdigit()));
    }
    assert_eq!(record["signature"].as_str().unwrap().len(), 128);
    assert_eq!(record["origin"]["span"]["start_line"], 1);
    assert_eq!(record["current"]["span"]["start_line"], 2);
    assert_eq!(record["name"], "charge");
    assert_eq!(record["operation"], "preserve");
    let map = fixture.read(".crane/map");
    assert!(
        map.contains(&format!(
            "{id} {}",
            record["origin"]["binding_digest"].as_str().unwrap()
        )),
        "{map}"
    );
    assert!(fixture
        .read(".crane/policies/default.crane")
        .contains(&format!("preserve {id};")));
    assert!(fixture.ok(&["test", "."]).contains("Crane test: PASS"));
}

/** Markers use each language's comment syntax, and the whole file is selected without lines
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn markers_follow_the_language() {
    let fixture = Fixture::ready();
    let rust = fixture.protect(&["src/lib.rs", "start-line", "5", "end-line", "7"]);
    assert!(fixture
        .read("src/lib.rs")
        .contains(&format!("// @crane:selection:{rust}:start\npub fn sub")));
    let js = fixture.target(&["web/app.js"]);
    let text = fixture.read("web/app.js");
    assert!(
        text.starts_with(&format!("// @crane:selection:{js}:start\n")),
        "{text}"
    );
    assert!(
        text.ends_with(&format!("// @crane:selection:{js}:end\n")),
        "{text}"
    );
    let markdown = fixture.protect(&["guide.md", "start-line", "3", "end-line", "3"]);
    assert!(fixture
        .read("docs/guide.md")
        .contains(&format!("<!-- @crane:selection:{markdown}:start -->")));
    let json = fixture.fails(&["protect", "data/config.json"]);
    assert!(json.contains("no supported comment syntax"), "{json}");
    assert!(fixture.ok(&["validate"]).contains("Crane validate: PASS"));
}

/** Multiple selections coexist, are independently addressable, and cannot overlap
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn multiple_selections_and_overlap() {
    let fixture = Fixture::ready();
    let first = fixture.protect(&["app/payments.py", "start-line", "1", "end-line", "4"]);
    let second = fixture.target(&["app/payments.py", "start-line", "9", "end-line", "10"]);
    assert_ne!(first, second);
    let overlap = fixture.fails(&[
        "protect",
        "app/payments.py",
        "start-line",
        "3",
        "end-line",
        "8",
    ]);
    assert!(overlap.contains("overlap"), "{overlap}");
    let markers = fixture.fails(&[
        "protect",
        "app/payments.py",
        "start-line",
        "1",
        "end-line",
        "1",
    ]);
    assert!(markers.contains("overlap"), "{markers}");
    let range = fixture.fails(&[
        "protect",
        "app/ledger.py",
        "start-line",
        "2",
        "end-line",
        "9",
    ]);
    assert!(range.contains("invalid line range"), "{range}");
    let (passed, report) = fixture.json(&["validate", "--json"]);
    assert!(passed);
    assert!(report["selections"][first.as_str()].is_object());
    assert!(report["selections"][second.as_str()].is_object());
}

#[test]
fn nested_selections_are_contained_and_project_child_content_from_parent() {
    let fixture = Fixture::ready();
    let parent = fixture.protect(&["app/payments.py", "start-line", "1", "end-line", "4"]);
    let child = fixture.target(&["app/payments.py", "start-line", "3", "end-line", "3"]);
    assert_ne!(parent, child);
    assert!(fixture.ok(&["validate"]).contains("Crane validate: PASS"));

    let text = fixture.read("app/payments.py");
    fixture.write(
        "app/payments.py",
        &text.replace("fee = amount * 0.03", "fee = amount * 0.04"),
    );
    assert!(
        fixture.ok(&["test", "."]).contains("Crane test: PASS"),
        "the nested target is editable without changing the parent's preserve fingerprint"
    );

    let partial = fixture.fails(&[
        "protect",
        "app/payments.py",
        "start-line",
        "4",
        "end-line",
        "8",
    ]);
    assert!(partial.contains("overlap"), "{partial}");

    let same_type = Fixture::ready();
    let outer = same_type.protect(&["app/payments.py", "start-line", "1", "end-line", "4"]);
    let inner = same_type.protect(&["app/payments.py", "start-line", "3", "end-line", "3"]);
    let text = same_type.read("app/payments.py");
    same_type.write(
        "app/payments.py",
        &text.replace("fee = amount * 0.03", "fee = amount * 0.04"),
    );
    let blocked = same_type.fails(&["test", "."]);
    assert!(
        blocked.contains(&outer) && blocked.contains(&inner),
        "{blocked}"
    );
}

/** Identifier collisions with the registry or existing markers are regenerated
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn identifier_collisions_regenerate() {
    let fixture = Fixture::ready();
    let first = fixture
        .command(&["protect", "app/ledger.py"])
        .env("CRANE_TEST_SELECTION_IDS", "AAAAAAAA")
        .output()
        .unwrap();
    assert!(first.status.success(), "{}", common::text(&first));
    let second = fixture
        .command(&["protect", "src/lib.rs", "start-line", "1", "end-line", "3"])
        .env("CRANE_TEST_SELECTION_IDS", "AAAAAAAA,BBBBBBBB")
        .output()
        .unwrap();
    let output = common::text(&second);
    assert!(second.status.success(), "{output}");
    assert!(output.contains("AAAAAAAA is already in use"), "{output}");
    assert_eq!(common::selection_id(&output), "BBBBBBBB");
}

/** A range that differs from the trusted checkpoint cannot be selected
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn ranges_resolve_against_the_checkpoint() {
    let fixture = Fixture::ready();
    fixture.write("app/payments.py", &PAYMENTS.replace("0.03", "0.05"));
    let changed = fixture.fails(&[
        "protect",
        "app/payments.py",
        "start-line",
        "1",
        "end-line",
        "4",
    ]);
    assert!(changed.contains("differ from checkpoint"), "{changed}");
    fixture.ok(&[
        "protect",
        "app/payments.py",
        "start-line",
        "7",
        "end-line",
        "8",
    ]);
    fixture.write("app/brand_new.py", "x = 1\n");
    let new_file = fixture.fails(&["protect", "app/brand_new.py"]);
    assert!(
        new_file.contains("does not exist in checkpoint"),
        "{new_file}"
    );
    let missing_checkpoint = fixture.fails(&["protect", "app/ledger.py", "checkpoint", "nope"]);
    assert!(
        missing_checkpoint.contains("does not exist"),
        "{missing_checkpoint}"
    );
}

/** Identity survives insertions above and below the selection and a file rename; a human
 * validate records the relocation as a new generation
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn identity_survives_shifts_and_renames() {
    let fixture = Fixture::ready();
    let id = fixture.protect(&["app/payments.py", "start-line", "1", "end-line", "4"]);
    let text = fixture.read("app/payments.py");
    fixture.write(
        "app/payments.py",
        &format!("import math\nimport os\n\n{text}\n\ndef extra():\n    pass\n"),
    );
    assert!(fixture.ok(&["test", "."]).contains("Crane test: PASS"));
    let generation = fixture.crane_json("registry.json")["manifest"]["generation"]
        .as_u64()
        .unwrap();
    git(&fixture.work, &["mv", "app/payments.py", "app/billing.py"]);
    let output = fixture.ok(&["validate"]);
    assert!(output.contains("recorded relocation"), "{output}");
    let moved = record(&fixture, &id);
    assert_eq!(moved["current"]["span"]["path"], "app/billing.py");
    assert_eq!(
        moved["origin"]["span"]["path"], "app/payments.py",
        "origin facts never change"
    );
    assert!(
        fixture.crane_json("registry.json")["manifest"]["generation"]
            .as_u64()
            .unwrap()
            > generation
    );
    assert!(fixture.ok(&["test", "."]).contains("Crane test: PASS"));
}

/** Changing preserved content fails its policy; restoring it passes again
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn modified_preserve_content_fails() {
    let fixture = Fixture::ready();
    fixture.protect(&["app/payments.py", "start-line", "1", "end-line", "4"]);
    let protected = fixture.read("app/payments.py");
    fixture.write("app/payments.py", &protected.replace("0.03", "0.30"));
    let output = fixture.fails(&["test", "."]);
    assert!(output.contains("FAIL policy default"), "{output}");
    assert!(
        output.contains("preserved selection changed since checkpoint baseline"),
        "{output}"
    );
    fixture.write("app/payments.py", &protected);
    assert!(fixture.ok(&["test", "."]).contains("PASS"));
}

/** Missing, unmatched, duplicated, malformed, and altered markers make a selection UNRESOLVED,
 * which fails closed
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn marker_problems_fail_closed() {
    let fixture = Fixture::ready();
    let id = fixture.protect(&["app/payments.py", "start-line", "1", "end-line", "4"]);
    let original = fixture.read("app/payments.py");
    let start = format!("# @crane:selection:{id}:start\n");
    let end = format!("# @crane:selection:{id}:end\n");
    let cases = [
        (
            "markers_missing",
            original.replace(&start, "").replace(&end, ""),
        ),
        ("marker_unmatched", original.replace(&end, "")),
        (
            "markers_duplicated",
            format!("{original}{start}x = 1\n{end}"),
        ),
        (
            "marker_malformed",
            original.replace(&start, &format!("#@crane:selection:{id}:start\n")),
        ),
        (
            "marker_inverted",
            original
                .replace(&start, "TEMP")
                .replace(&end, &start)
                .replace("TEMP", &end),
        ),
    ];
    for (expected, content) in cases {
        fixture.write("app/payments.py", &content);
        let output = fixture.fails(&["test", "."]);
        assert!(output.contains("UNRESOLVED"), "{expected}: {output}");
        let validate = fixture.fails(&["validate", "--json"]);
        assert!(
            validate.contains("selection_unresolved") || validate.contains("marker_malformed"),
            "{expected}: {validate}"
        );
    }
    fixture.write("app/payments.py", &original);
    fixture.write("app/copy.py", &format!("{start}y = 2\n{end}"));
    let duplicated = fixture.fails(&["test", "."]);
    assert!(duplicated.contains("several files"), "{duplicated}");
    fs::remove_file(fixture.work.join("app/copy.py")).unwrap();
    fs::remove_file(fixture.work.join("app/payments.py")).unwrap();
    fixture.write("app/elsewhere.py", PAYMENTS);
    let removed = fixture.fails(&["validate"]);
    assert!(
        removed.contains("never rebound automatically"),
        "candidate content is reported, not bound: {removed}"
    );
}

/** A marker that is not in the signed registry is reported and grants nothing
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn forged_markers_are_detected() {
    let fixture = Fixture::ready();
    fixture.write("app/ledger.py", "# @crane:selection:ZZZZ9999:start\ndef post(entry):\n    return entry\n# @crane:selection:ZZZZ9999:end\n");
    let output = fixture.fails(&["validate"]);
    assert!(output.contains("marker_unregistered"), "{output}");
}

#[test]
fn crossed_nested_markers_fail_validation() {
    let fixture = Fixture::ready();
    fixture.write(
        "app/ledger.py",
        "# @crane:selection:AAAAAAAA:start\n# @crane:selection:BBBBBBBB:start\nx = 1\n# @crane:selection:AAAAAAAA:end\n# @crane:selection:BBBBBBBB:end\n",
    );
    let output = fixture.fails(&["validate"]);
    assert!(output.contains("marker_crossed"), "{output}");
}

/** Editing a registry record, the map, the configuration, or the checkpoints is detected
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn registry_tampering_is_detected() {
    let fixture = Fixture::ready();
    let id = fixture.protect(&["app/payments.py", "start-line", "1", "end-line", "4"]);
    let registry = fixture.read(".crane/registry.json");
    let tamper = |file: &str, from: &str, to: &str, code: &str| {
        let original = fixture.read(file);
        assert!(original.contains(from), "{file} contains {from}");
        fixture.write(file, &original.replacen(from, to, 1));
        let output = fixture.fails(&["validate"]);
        assert!(output.contains(code), "{file}: expected {code}: {output}");
        fixture.write(file, &original);
    };
    tamper(
        ".crane/registry.json",
        "\"operation\": \"preserve\"",
        "\"operation\": \"target\"",
        "selection_record_invalid",
    );
    let record = record(&fixture, &id);
    let signature = record["signature"].as_str().unwrap();
    let flipped = format!(
        "{}{}",
        if signature.starts_with('0') { "1" } else { "0" },
        &signature[1..]
    );
    tamper(
        ".crane/registry.json",
        signature,
        &flipped,
        "invalid or untrusted signature",
    );
    tamper(".crane/map", &id, "QQQQQQQQ", "map_tampered");
    tamper(
        ".crane/config.json",
        "\"default_policy\": \"default\"",
        "\"default_policy\": \"evil\"",
        "governance_file_tampered",
    );
    tamper(
        ".crane/checkpoints.json",
        "\"baseline\"",
        "\"baseline2\"",
        "checkpoints_tampered",
    );
    fixture.write(".crane/registry.json", &registry);
    assert!(fixture.ok(&["validate"]).contains("PASS"));
    let protect_refused = {
        fixture.write(".crane/map", "");
        let output = fixture.fails(&["protect", "app/ledger.py"]);
        fixture.ok(&["--version"]);
        output
    };
    assert!(
        protect_refused.contains("fails verification"),
        "{protect_refused}"
    );
}

/** Rolling the registry back to an older generation, or deleting it, is detected
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn rollback_and_deletion_are_detected() {
    let fixture = Fixture::ready();
    let saved = [
        "registry.json",
        "map",
        "audit.jsonl",
        "config.json",
        "checkpoints.json",
        "context.json",
    ]
    .map(|name| (name, fixture.read(&format!(".crane/{name}"))));
    let policy = fixture.read(".crane/policies/default.crane");
    fixture.protect(&["app/ledger.py"]);
    for (name, content) in &saved {
        fixture.write(&format!(".crane/{name}"), content);
    }
    fixture.write(".crane/policies/default.crane", &policy);
    fixture.write("app/ledger.py", "def post(entry):\n    return entry\n");
    let output = fixture.fails(&["validate"]);
    assert!(output.contains("registry_rollback"), "{output}");
    fs::remove_file(fixture.work.join(".crane/registry.json")).unwrap();
    let deleted = fixture.fails(&["validate"]);
    assert!(deleted.contains("registry_missing"), "{deleted}");
}

/** A registry copied into another repository, or verified by a machine without a trusted key,
 * is not accepted
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn registries_do_not_transfer() {
    let fixture = Fixture::ready();
    fixture.protect(&["app/payments.py", "start-line", "1", "end-line", "4"]);
    let other = Fixture::ready();
    for name in [
        "registry.json",
        "map",
        "audit.jsonl",
        "config.json",
        "checkpoints.json",
        "context.json",
    ] {
        other.write(
            &format!(".crane/{name}"),
            &fixture.read(&format!(".crane/{name}")),
        );
    }
    other.write(
        ".crane/policies/default.crane",
        &fixture.read(".crane/policies/default.crane"),
    );
    other.write("app/payments.py", &fixture.read("app/payments.py"));
    let output = other.fails(&["validate"]);
    assert!(
        output.contains("registry_wrong_repository")
            || output.contains("untrusted")
            || output.contains("another repository"),
        "{output}"
    );
    let stranger = fixture
        .command(&["validate"])
        .env("CRANE_HOME", fixture.root.path().join("empty-trust"))
        .output()
        .unwrap();
    assert!(!stranger.status.success());
    assert!(
        common::text(&stranger).contains("no_trusted_key"),
        "{}",
        common::text(&stranger)
    );
    let public = fixture.crane_json("registry.json")["manifest"]["public_key"]
        .as_str()
        .unwrap()
        .to_string();
    let trusted = fixture
        .command(&["validate"])
        .env("CRANE_HOME", fixture.root.path().join("ci-trust"))
        .env("CRANE_TRUSTED_PUBLIC_KEYS", &public)
        .output()
        .unwrap();
    assert!(
        trusted.status.success(),
        "a CI machine with the public key verifies: {}",
        common::text(&trusted)
    );
}
