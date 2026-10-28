// Unit tests of the Brick module: the closed type set, typed configuration and its validation,
// Stack validation, monotonic composition (nothing loosens, pins Crane cannot honor fail closed), folding
// controls into the guard's limits (tighten only), deterministic context snapshots with their
// sources, and the per-session delivery ledger.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::json;

use super::compose::{controls, file_limit, scope_resolved};
use super::config::{self, Config};
use super::context::{assemble, deliver, reset, Delivery};
use super::model::{
    Attachment, AttachmentState, Author, Brick, BrickEntry, BrickRef, CurrentContext, Stack,
    StackEntry, Subject, Target, TargetKind, ValidationStatus, VersionState,
};
use super::store::{new_brick, BrickStore};
use super::types::BrickType;
use super::validate;
use crate::governance::state::State;
use crate::governance::workspace::Workspace;
use crate::governance::zones::{Ceiling, Criticality, Lifecycle, Limit, Limits, Resolved};

/** A Brick definition of a type with a configuration and common context
 * Input
    - id: &str - Brick id
    - version: u32 - version
    - brick_type: BrickType - type
    - config: serde_json::Value - configuration
    - context: &str - common context
 * Output
    - Brick
*/
fn brick(
    id: &str,
    version: u32,
    brick_type: BrickType,
    config: serde_json::Value,
    context: &str,
) -> Brick {
    let mut brick = new_brick(id, version, brick_type, id, Author::default());
    brick.intent = format!("{id} intent");
    brick.config = config;
    brick.common_context.text = context.into();
    brick.created_at = 1;
    brick
}

/** An activated entry of a Brick in a state
 * Input
    - brick: Brick - definition
    - state: VersionState - state
 * Output
    - BrickEntry
*/
fn entry(brick: Brick, state: VersionState) -> BrickEntry {
    BrickEntry {
        digest: brick.digest(),
        brick,
        state,
        validation: Default::default(),
        activated_by: Author::default(),
        activated_at: 1,
        reason: "test".into(),
        self_reviewed: false,
        changed_by: None,
        changed_at: None,
        change_reason: None,
    }
}

/** An enabled attachment of a subject to a target
 * Input
    - id: &str - attachment id
    - target: &str - "flow:ID" or "zone:ID"
    - subject: &str - "ID@N" or "stack:ID@N"
    - context: &str - Current Context
 * Output
    - Attachment
*/
fn attached(id: &str, target: &str, subject: &str, context: &str) -> Attachment {
    Attachment {
        attachment_id: id.into(),
        target: Target::parse(target).unwrap(),
        subject: Subject::parse(subject).unwrap(),
        current_context: CurrentContext {
            text: context.into(),
            updated_by: Author::default(),
            updated_at: 1,
        },
        state: AttachmentState::Enabled,
        attached_by: Author::default(),
        attached_at: 1,
    }
}

/** A workspace over a directory, for functions that only read files from its root
 * Input
    - root: &Path - directory
 * Output
    - Workspace
*/
fn workspace(root: &Path) -> Workspace {
    Workspace {
        root: root.to_path_buf(),
        crane: root.join(".crane"),
        repository_id: "test".into(),
        remote: None,
        root_commits: Vec::new(),
        user: None,
        email: None,
        identity_cache: "NONE",
    }
}

/** The type set is closed: 12 current kinds whose names round-trip through serde, each with one
 * purpose and one unique property; former kinds stay readable but cannot be created; unknown names
 * are refused
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn the_type_set_is_closed() {
    assert_eq!(BrickType::ALL.len(), 12);
    for former in BrickType::LEGACY {
        assert_eq!(json!(former), json!(former.name()), "{former:?}");
        assert!(BrickType::parse(former.name())
            .unwrap_err()
            .contains("former"));
        assert_eq!(BrickType::parse_any(former.name()).unwrap(), former);
        assert!(former.successors().iter().all(|kind| !kind.is_legacy()));
    }
    let mut purposes = std::collections::BTreeSet::new();
    for brick_type in BrickType::ALL {
        let info = brick_type.catalog();
        assert!(
            !info.purpose.contains('\n') && !info.unique.contains('\n'),
            "{brick_type:?}: one line each"
        );
        assert!(
            purposes.insert(info.unique),
            "{brick_type:?}: its unique property is shared"
        );
        assert_eq!(
            json!(brick_type),
            json!(brick_type.name()),
            "{brick_type:?}"
        );
        assert_eq!(
            BrickType::parse(&brick_type.name().to_ascii_uppercase()).unwrap(),
            brick_type
        );
        assert!(!brick_type.catalog().runtime.is_empty());
    }
    assert!(BrickType::parse("custom")
        .unwrap_err()
        .contains("the set is fixed"));
    assert!(serde_json::from_value::<BrickType>(json!("custom")).is_err());
    // A definition of an unknown type does not even parse
    let mut value =
        serde_json::to_value(brick("a", 1, BrickType::Knowledge, json!({}), "x")).unwrap();
    value["type"] = json!("data_protection");
    assert!(serde_json::from_value::<Brick>(value).is_err());
}

/** Configurations are typed: unknown fields (a command, a script) are refused, and each type's
 * rules hold
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn configurations_are_typed_and_validated() {
    let adapters = vec![("git".to_string(), "git.commit".to_string())];
    let profiles = vec![
        ("agent-hooks".to_string(), true),
        ("sandbox".to_string(), false),
    ];
    let codes = |brick_type: BrickType, value: serde_json::Value| -> Vec<String> {
        match config::parse(brick_type, &value) {
            Err(problem) => vec![problem.code],
            Ok(parsed) => parsed
                .iter()
                .flat_map(|config| config::validate(config, &adapters, &profiles))
                .map(|problem| problem.code)
                .collect(),
        }
    };
    // No arbitrary execution: an action names a registered operation, nothing else
    assert_eq!(
        codes(
            BrickType::Action,
            json!({"adapter": "git", "operation": "git.commit", "command": "rm -rf /"})
        ),
        ["CONFIG_INVALID"]
    );
    assert_eq!(
        codes(
            BrickType::Action,
            json!({"adapter": "shell", "operation": "run"})
        ),
        ["ACTION_NOT_REGISTERED"]
    );
    assert_eq!(
        codes(
            BrickType::Action,
            json!({"adapter": "git", "operation": "git commit -a"})
        ),
        ["ACTION_NOT_REGISTERED"]
    );
    assert!(codes(
        BrickType::Action,
        json!({"adapter": "git", "operation": "git.commit"})
    )
    .is_empty());
    // Scope: never empty, never outside the repository
    assert_eq!(
        codes(BrickType::Scope, json!({"include": []})),
        ["SCOPE_EMPTY"]
    );
    assert_eq!(
        codes(BrickType::Scope, json!({"include": ["../etc/**"]})),
        ["PATH_OUTSIDE_REPOSITORY"]
    );
    assert_eq!(
        codes(BrickType::Scope, json!({"include": ["C:/x"]})),
        ["PATH_OUTSIDE_REPOSITORY"]
    );
    assert!(codes(BrickType::Scope, json!({"include": ["app/**"]})).is_empty());
    // Preconditions stay cheap; verification may run the policy tests
    assert_eq!(
        codes(
            BrickType::Precondition,
            json!({"checks": [{"kind": "policy_tests_pass"}]})
        ),
        ["PRECONDITION_TOO_COSTLY"]
    );
    assert!(codes(
        BrickType::Verification,
        json!({"checks": [{"kind": "policy_tests_pass"}]})
    )
    .is_empty());
    // Isolation reports an unavailable profile instead of pretending
    assert_eq!(
        codes(BrickType::Isolation, json!({"profile": "sandbox"})),
        ["ISOLATION_UNAVAILABLE"]
    );
    assert_eq!(
        codes(BrickType::Isolation, json!({"profile": "docker"})),
        ["ISOLATION_UNKNOWN_PROFILE"]
    );
    // Recovery reconciles before retrying; intelligence only advises; monitors are not loops
    assert_eq!(
        codes(
            BrickType::Recovery,
            json!({"max_retries": 2, "reconcile_before_retry": false})
        ),
        ["RECOVERY_UNSAFE_RETRY"]
    );
    assert_eq!(
        codes(
            BrickType::Intelligence,
            json!({"analyses": ["denial_patterns"], "advisory_only": false})
        ),
        ["INTELLIGENCE_NOT_ADVISORY"]
    );
    assert_eq!(
        codes(
            BrickType::Monitoring,
            json!({"signals": [{"source": "policy_tests", "every_minutes": 1}]})
        ),
        ["MONITOR_TOO_FREQUENT"]
    );
    assert_eq!(
        codes(
            BrickType::Monitoring,
            json!({"signals": [{"source": "denials", "every_minutes": 10}]})
        ),
        ["MONITOR_THRESHOLD_MISSING"]
    );
    assert_eq!(
        codes(BrickType::Approval, json!({"operations": ["read"]})),
        ["APPROVAL_READ_UNSUPPORTED"]
    );
    assert_eq!(
        codes(BrickType::Budget, json!({"max_tool_calls": 0})),
        ["LIMITS_ZERO"]
    );
    assert_eq!(
        codes(BrickType::Constraint, json!({})),
        ["CONSTRAINT_EMPTY"]
    );
    // A former constraint is read as limits (and access, for protected paths)
    assert!(matches!(
        config::parse(BrickType::Constraint, &json!({"max_ceiling": "observe"})).as_deref(),
        Ok([Config::Limits(_)])
    ));
    assert!(matches!(
        config::parse(
            BrickType::Constraint,
            &json!({"max_ceiling": "observe", "protected_paths": ["db/**"]})
        )
        .as_deref(),
        Ok([Config::Limits(_), Config::Access(_)])
    ));
}

/** Validation refuses secrets in context, unresolved or revoked dependencies, and a missing intent
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn validation_refuses_secrets_and_bad_dependencies() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = BrickStore::default();
    store.bricks.push(entry(
        brick("old", 1, BrickType::Knowledge, json!({}), "x"),
        VersionState::Revoked,
    ));
    let ok = validate::brick(
        &brick(
            "guide",
            1,
            BrickType::Knowledge,
            json!({}),
            "Payments are idempotent.",
        ),
        &store,
        directory.path(),
    );
    assert_eq!(ok.status, ValidationStatus::Passed, "{:?}", ok.problems);
    let secret = validate::brick(
        &brick(
            "guide",
            1,
            BrickType::Knowledge,
            json!({}),
            "token ghp_abcdefghijklmnopqrstuvwxyz0123456789",
        ),
        &store,
        directory.path(),
    );
    assert!(secret
        .problems
        .iter()
        .any(|problem| problem.code == "CONTEXT_HOLDS_SECRET"));
    let mut dependent = brick("guide", 1, BrickType::Knowledge, json!({}), "x");
    dependent.dependencies = vec![
        BrickRef {
            brick_id: "old".into(),
            version: 1,
        },
        BrickRef {
            brick_id: "none".into(),
            version: 1,
        },
        BrickRef {
            brick_id: "guide".into(),
            version: 1,
        },
    ];
    dependent.intent.clear();
    let codes = validate::brick(&dependent, &store, directory.path())
        .problems
        .into_iter()
        .map(|problem| problem.code)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        codes,
        BTreeSet::from([
            "DEPENDENCY_REVOKED".to_string(),
            "DEPENDENCY_UNRESOLVED".into(),
            "DEPENDENCY_CYCLE".into(),
            "INTENT_MISSING".into()
        ])
    );
    // An unavailable isolation profile blocks rather than fails
    let blocked = validate::brick(
        &brick(
            "iso",
            1,
            BrickType::Environment,
            json!({"profile": "sandbox"}),
            "",
        ),
        &store,
        directory.path(),
    );
    assert_eq!(blocked.status, ValidationStatus::Blocked);
}

/** Stacks hold one exact, active version per Brick, carry their members' dependencies, and keep
 * one isolation profile
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn stacks_validate_their_members() {
    let mut store = BrickStore::default();
    let mut needs = brick(
        "needs",
        1,
        BrickType::Scope,
        json!({"include": ["app/**"]}),
        "",
    );
    needs.dependencies = vec![BrickRef {
        brick_id: "base".into(),
        version: 1,
    }];
    store.bricks.push(entry(needs, VersionState::Active));
    store.bricks.push(entry(
        brick("base", 1, BrickType::Knowledge, json!({}), "x"),
        VersionState::Active,
    ));
    store.bricks.push(entry(
        brick("base", 2, BrickType::Knowledge, json!({}), "y"),
        VersionState::Active,
    ));
    store.bricks.push(entry(
        brick(
            "hooks",
            1,
            BrickType::Isolation,
            json!({"profile": "agent-hooks"}),
            "",
        ),
        VersionState::Active,
    ));
    store.bricks.push(entry(
        brick(
            "adapters",
            1,
            BrickType::Isolation,
            json!({"profile": "typed-adapters"}),
            "",
        ),
        VersionState::Active,
    ));
    let stack = |members: &[(&str, u32)]| Stack {
        stack_id: "prod".into(),
        version: 1,
        name: "Production change".into(),
        purpose: String::new(),
        owner: None,
        members: members
            .iter()
            .map(|(id, version)| BrickRef {
                brick_id: (*id).into(),
                version: *version,
            })
            .collect(),
        created_by: Author::default(),
        created_at: 1,
    };
    let codes = |members: &[(&str, u32)]| {
        validate::stack(&stack(members), &store)
            .problems
            .into_iter()
            .map(|problem| problem.code)
            .collect::<Vec<_>>()
    };
    assert!(codes(&[("needs", 1), ("base", 1)]).is_empty());
    assert_eq!(codes(&[("needs", 1)]), ["STACK_DEPENDENCY_MISSING"]);
    assert_eq!(
        codes(&[("base", 1), ("base", 2)]),
        ["STACK_DUPLICATE_MEMBER"]
    );
    assert_eq!(
        codes(&[("hooks", 1), ("adapters", 1)]),
        ["STACK_ISOLATION_CONFLICT"]
    );
    assert_eq!(
        codes(&[("prod", 1)]),
        ["DEPENDENCY_CYCLE"],
        "a Stack never contains itself"
    );
    assert_eq!(codes(&[]), ["STACK_EMPTY"]);
}

/** Controls combine monotonically: the lowest ceiling and highest criticality win, a Stack cannot
 * loosen its members, and pins Crane cannot honor fail closed
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn controls_only_tighten_and_fail_closed() {
    let root = Path::new(".");
    let mut store = BrickStore::default();
    store.bricks.push(entry(
        brick(
            "loose",
            1,
            BrickType::Constraint,
            json!({"max_ceiling": "autonomous"}),
            "",
        ),
        VersionState::Active,
    ));
    store.bricks.push(entry(brick("strict", 1, BrickType::Constraint, json!({"max_ceiling": "assisted", "min_criticality": "critical", "protected_paths": ["app/ledger.py"]}), ""), VersionState::Active));
    store.bricks.push(entry(
        brick(
            "scope",
            1,
            BrickType::Scope,
            json!({"include": ["app/pay*.py"]}),
            "",
        ),
        VersionState::Active,
    ));
    store.bricks.push(entry(
        brick(
            "gone",
            1,
            BrickType::Constraint,
            json!({"max_ceiling": "observe"}),
            "",
        ),
        VersionState::Revoked,
    ));
    store.stacks.push(StackEntry {
        digest: String::new(),
        stack: Stack {
            stack_id: "mix".into(),
            version: 1,
            name: "Mix".into(),
            purpose: String::new(),
            owner: None,
            members: vec![
                BrickRef {
                    brick_id: "loose".into(),
                    version: 1,
                },
                BrickRef {
                    brick_id: "strict".into(),
                    version: 1,
                },
            ],
            created_by: Author::default(),
            created_at: 1,
        },
        state: VersionState::Active,
        activated_by: Author::default(),
        activated_at: 1,
        reason: String::new(),
        changed_by: None,
        changed_at: None,
        change_reason: None,
    });
    store.attachments = vec![
        attached("a1", "flow:checkout", "stack:mix@1", ""),
        attached("a2", "flow:checkout", "scope@1", ""),
    ];
    let combined = controls(&store, "flow:checkout", root);
    assert_eq!(
        combined.ceiling,
        Some(Ceiling::Assisted),
        "the stricter member wins inside a Stack"
    );
    assert_eq!(combined.criticality, Some(Criticality::Critical));
    let files = BTreeSet::from([
        "app/payments.py".to_string(),
        "app/ledger.py".into(),
        "app/service.py".into(),
    ]);
    let resolved = scope_resolved(&combined, &files);
    assert!(resolved);
    assert_eq!(
        file_limit(&combined, "app/payments.py", resolved).0,
        Some(Ceiling::Assisted)
    );
    assert_eq!(
        file_limit(&combined, "app/ledger.py", resolved).0,
        Some(Ceiling::Observe),
        "protected path"
    );
    assert_eq!(
        file_limit(&combined, "app/service.py", resolved).0,
        Some(Ceiling::Observe),
        "outside the scope"
    );
    // A scope that matches nothing never widens: everything becomes read-only
    let none = BTreeSet::from(["web/app.js".to_string()]);
    assert!(!scope_resolved(&combined, &none));
    assert_eq!(
        file_limit(&combined, "web/app.js", false).0,
        Some(Ceiling::Observe)
    );
    // A revoked pin and an unknown Stack fail closed (approval), never disappear
    store.attachments = vec![
        attached("a3", "zone:ledger", "gone@1", ""),
        attached("a4", "zone:ledger", "stack:missing@1", ""),
    ];
    let closed = controls(&store, "zone:ledger", root);
    assert_eq!(closed.fail_closed.len(), 2, "{:?}", closed.fail_closed);
    assert_eq!(
        file_limit(&closed, "app/ledger.py", true).0,
        Some(Ceiling::Assisted)
    );
    // Approval, a failing precondition, and an unavailable isolation profile restrict too
    store.bricks.push(entry(
        brick(
            "ask",
            1,
            BrickType::Approval,
            json!({"operations": ["write"]}),
            "",
        ),
        VersionState::Active,
    ));
    store.bricks.push(entry(
        brick(
            "pre",
            1,
            BrickType::Precondition,
            json!({"checks": [{"kind": "file_exists", "path": "no/such/file"}]}),
            "",
        ),
        VersionState::Active,
    ));
    store.bricks.push(entry(
        brick(
            "box",
            1,
            BrickType::Isolation,
            json!({"profile": "sandbox"}),
            "",
        ),
        VersionState::Active,
    ));
    for (subject, expected) in [
        ("ask@1", Ceiling::Assisted),
        ("pre@1", Ceiling::Assisted),
        ("box@1", Ceiling::Observe),
    ] {
        store.attachments = vec![attached("a5", "zone:z", subject, "")];
        let combined = controls(&store, "zone:z", root);
        assert_eq!(
            file_limit(&combined, "x.py", true).0,
            Some(expected),
            "{subject}"
        );
    }
    // Context and declared types restrict nothing
    store.bricks.push(entry(
        brick("note", 1, BrickType::Knowledge, json!({}), "x"),
        VersionState::Active,
    ));
    store.attachments = vec![attached("a6", "zone:z", "note@1", "")];
    assert!(!controls(&store, "zone:z", root).restricts());
}

/** Folding Bricks into the guard's limits lowers ceilings and binds the Brick, but never raises a
 * ceiling, and touches only the attached target's files
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn limits_are_tightened_never_widened() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = BrickStore::default();
    store.bricks.push(entry(
        brick(
            "cap",
            1,
            BrickType::Constraint,
            json!({"max_ceiling": "autonomous", "min_criticality": "sensitive"}),
            "",
        ),
        VersionState::Active,
    ));
    store.bricks.push(entry(
        brick(
            "ro",
            1,
            BrickType::Permission,
            json!({"allow": ["read"]}),
            "",
        ),
        VersionState::Active,
    ));
    store.attachments = vec![
        attached("a1", "zone:pay", "cap@1", ""),
        attached("a2", "flow:post", "ro@1", ""),
    ];
    let mut state = State::default();
    store.clone().save_into(&mut state).unwrap();
    let limit = |ceiling| Limit {
        ceiling,
        criticality: Criticality::Routine,
        entities: Vec::new(),
        scopes: Vec::new(),
        bricks: Vec::new(),
    };
    let mut limits = Limits::new();
    limits.insert("app/payments.py".into(), limit(Ceiling::Assisted));
    limits.insert("app/ledger.py".into(), limit(Ceiling::Delegated));
    limits.insert("web/app.js".into(), limit(Ceiling::Delegated));
    let resolved = |kind, id: &str, file: &str| Resolved {
        kind,
        id: id.into(),
        status: Lifecycle::Active,
        criticality: Criticality::Routine,
        ceiling: Ceiling::Delegated,
        files: BTreeSet::from([file.to_string()]),
        graph: None,
    };
    let entities = vec![
        resolved("zone", "pay", "app/payments.py"),
        resolved("flow", "post", "app/ledger.py"),
    ];
    super::effect::apply(&workspace(directory.path()), &state, &entities, &mut limits);
    assert_eq!(
        limits["app/payments.py"].ceiling,
        Ceiling::Assisted,
        "an 'autonomous' cap never raises assisted"
    );
    assert_eq!(
        limits["app/payments.py"].criticality,
        Criticality::Sensitive
    );
    assert_eq!(
        limits["app/ledger.py"].ceiling,
        Ceiling::Observe,
        "read-only permission"
    );
    assert_eq!(limits["app/ledger.py"].bricks, ["brick:ro@1"]);
    assert_eq!(
        limits["web/app.js"].ceiling,
        Ceiling::Delegated,
        "no attachment, no change"
    );
    assert!(limits["web/app.js"].bricks.is_empty());
}

/** Snapshots are deterministic, label every source, keep Current Context to its attachment, report
 * version conflicts, and change digest when any input changes
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn snapshots_are_deterministic_and_sourced() {
    let directory = tempfile::tempdir().unwrap();
    let workspace = workspace(directory.path());
    let mut store = BrickStore::default();
    store.bricks.push(entry(
        brick(
            "idem",
            1,
            BrickType::Knowledge,
            json!({}),
            "Charges are idempotent.",
        ),
        VersionState::Active,
    ));
    store.bricks.push(entry(
        brick(
            "idem",
            2,
            BrickType::Knowledge,
            json!({}),
            "Charges are idempotent; refunds too.",
        ),
        VersionState::Active,
    ));
    store.bricks.push(entry(
        brick(
            "cap",
            1,
            BrickType::Constraint,
            json!({"max_ceiling": "assisted"}),
            "Ask a person.",
        ),
        VersionState::Active,
    ));
    store.attachments = vec![
        attached(
            "a1",
            "flow:checkout",
            "idem@1",
            "Checkout uses the v1 gateway.",
        ),
        attached("a2", "flow:refund", "idem@2", "Refunds go through finance."),
        attached("a3", "flow:checkout", "cap@1", ""),
    ];
    let state = State::default();
    let one = BTreeSet::from(["flow:checkout".to_string()]);
    let first = assemble(&workspace, &state, &store, None, &one);
    let again = assemble(&workspace, &state, &store, None, &one);
    assert_eq!(first.digest, again.digest, "same inputs, same digest");
    assert_eq!(
        first
            .sections
            .iter()
            .map(|section| section.kind.as_str())
            .collect::<Vec<_>>(),
        [
            "enforcement",
            "brick_common",
            "brick_current",
            "brick_common"
        ]
    );
    let text = first
        .sections
        .iter()
        .map(|section| section.text.clone())
        .collect::<String>();
    assert!(
        text.contains("Checkout uses the v1 gateway.")
            && !text.contains("Refunds go through finance."),
        "Current Context stays on its attachment"
    );
    assert!(
        text.contains("autonomy ceiling at most assisted"),
        "enforced controls are stated as facts"
    );
    assert!(first.conflicts.is_empty());
    let both = assemble(
        &workspace,
        &state,
        &store,
        None,
        &BTreeSet::from(["flow:checkout".to_string(), "flow:refund".into()]),
    );
    assert!(both
        .conflicts
        .iter()
        .any(|conflict| conflict.code == "BRICK_VERSION_CONFLICT"));
    // Changing one attachment's Current Context changes that snapshot only
    store.attachments[1].current_context.text = "Refunds need two approvals.".into();
    assert_eq!(
        assemble(&workspace, &state, &store, None, &one).digest,
        first.digest
    );
    store.attachments[0].current_context.text = "Checkout uses the v2 gateway.".into();
    assert_ne!(
        assemble(&workspace, &state, &store, None, &one).digest,
        first.digest
    );
    // A revoked version contributes no context and is reported
    store.bricks[0].state = VersionState::Revoked;
    let revoked = assemble(&workspace, &state, &store, None, &one);
    assert!(revoked
        .conflicts
        .iter()
        .any(|conflict| conflict.code == "PIN_NOT_HONORED"));
    assert!(!revoked.bricks.contains(&"idem@1".to_string()));
}

/** A session receives a snapshot once; an unchanged one is referenced, a changed one is resent, and
 * a reset session receives everything again
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn delivery_is_once_per_session_while_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let workspace = workspace(directory.path());
    let mut store = BrickStore::default();
    store.bricks.push(entry(
        brick("idem", 1, BrickType::Knowledge, json!({}), "Idempotent."),
        VersionState::Active,
    ));
    store.attachments = vec![attached("a1", "zone:pay", "idem@1", "")];
    let targets = BTreeSet::from(["zone:pay".to_string()]);
    let snapshot = assemble(&workspace, &State::default(), &store, None, &targets);
    let runtime = directory.path();
    assert_eq!(deliver(runtime, "s1", &snapshot, 10), Delivery::Delivered);
    assert_eq!(deliver(runtime, "s1", &snapshot, 10), Delivery::Reused);
    assert_eq!(
        deliver(runtime, "s2", &snapshot, 10),
        Delivery::Delivered,
        "per session"
    );
    store.attachments[0].current_context.text = "new".into();
    let changed = assemble(&workspace, &State::default(), &store, None, &targets);
    assert_eq!(
        deliver(runtime, "s1", &changed, 10),
        Delivery::Delivered,
        "a change is resent"
    );
    reset(runtime, "s1");
    assert_eq!(
        deliver(runtime, "s1", &changed, 10),
        Delivery::Delivered,
        "a reset session gets it again"
    );
}

/** References are exact: subjects need a version, targets a known kind
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn references_are_exact() {
    assert_eq!(Subject::parse("guard@2").unwrap().text(), "brick:guard@2");
    assert_eq!(
        Subject::parse("stack:prod@v3").unwrap().text(),
        "stack:prod@3"
    );
    assert!(Subject::parse("guard").is_err());
    assert!(Subject::parse("guard@0").is_err());
    assert_eq!(
        Target::parse("runbook:deploy").unwrap().kind,
        TargetKind::Runbook
    );
    assert!(Target::parse("policy:x").is_err());
    assert!(Target::parse("flow:../x").is_err());
}

/** The rules decided per call: identity, unreadable files, operation modes, kinds of change, caps,
 * hidden variables, network reach, and direct operations; each only makes a decision stricter
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn per_call_rules_only_tighten() {
    use super::guard::{decide, Call, Change, Rules};
    use super::usage::{TargetUsage, Usage};
    use crate::hooks::action::Operation;
    use crate::telemetry::model::Decision;

    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let mut store = BrickStore::default();
    for (id, kind, config) in [
        ("who", BrickType::Identity, json!({"agents": ["claude"]})),
        (
            "files",
            BrickType::Access,
            json!({"deny_read": ["app/secrets/**"], "delete": "deny", "create": "ask"}),
        ),
        (
            "human",
            BrickType::Approval,
            json!({"operations": ["dependency_change", "git_push"], "reason": "release owners decide"}),
        ),
        ("caps", BrickType::Limits, json!({"max_tool_calls": 2})),
        (
            "env",
            BrickType::Environment,
            json!({"hide_env": ["AWS_*"], "network": {"allow": ["api.example.com"]}}),
        ),
        (
            "ops",
            BrickType::Action,
            json!({"operations": [{"adapter": "git", "operation": "git.push"}], "deny_direct": true}),
        ),
    ] {
        store
            .bricks
            .push(entry(brick(id, 1, kind, config, ""), VersionState::Active));
    }
    store.attachments = ["who", "files", "human", "caps", "env", "ops"]
        .iter()
        .enumerate()
        .map(|(index, id)| attached(&format!("a{index}"), "zone:pay", &format!("{id}@1"), ""))
        .collect();
    let rules = Rules {
        targets: std::collections::BTreeMap::from([(
            "zone:pay".to_string(),
            controls(&store, "zone:pay", root),
        )]),
        paused: BTreeSet::new(),
    };
    let mut limits = Limits::new();
    for file in ["app/pay.py", "app/package.json", "app/secrets/key.txt"] {
        limits.insert(
            file.into(),
            Limit {
                ceiling: Ceiling::Delegated,
                criticality: Criticality::Routine,
                entities: vec!["zone:pay".into()],
                scopes: Vec::new(),
                bricks: Vec::new(),
            },
        );
    }
    let workspace = workspace(root);
    let no_args: Vec<String> = Vec::new();
    let no_hosts: Vec<String> = Vec::new();
    let call = |operation,
                reads: &[&str],
                changes: Vec<Change>,
                command: Option<&'static str>,
                agent: &'static str| Call {
        operation,
        reads: reads.iter().map(|read| read.to_string()).collect(),
        changes,
        named: Vec::new(),
        command,
        arguments: &no_args,
        network: &no_hosts,
        agent: Some(agent),
        agent_id: None,
        model: None,
        started_at: None,
        usage: None,
        now: 0,
    };
    let rules_of = |outcome: &super::guard::Outcome| {
        outcome
            .findings
            .iter()
            .map(|finding| (finding.rule, finding.decision))
            .collect::<Vec<_>>()
    };
    let change = |path: &str, existed, deleted| Change {
        path: path.into(),
        existed,
        deleted,
        lines: 1,
    };
    // Identity: only claude works in zone:pay
    let read = call(Operation::Read, &["app/pay.py"], Vec::new(), None, "codex");
    assert_eq!(
        rules_of(&decide(&rules, &limits, &workspace, &read)),
        [("BRICK_IDENTITY", Decision::Deny)]
    );
    let read = call(Operation::Read, &["app/pay.py"], Vec::new(), None, "claude");
    assert!(decide(&rules, &limits, &workspace, &read)
        .findings
        .is_empty());
    // Access: unreadable files, deletes denied, new files (attributed by their folder) asked
    let secret = call(
        Operation::Read,
        &["app/secrets/key.txt"],
        Vec::new(),
        None,
        "claude",
    );
    assert_eq!(
        rules_of(&decide(&rules, &limits, &workspace, &secret)),
        [("BRICK_DENY_READ", Decision::Deny)]
    );
    let delete = call(
        Operation::Write,
        &[],
        vec![change("app/pay.py", true, true)],
        None,
        "claude",
    );
    assert_eq!(
        rules_of(&decide(&rules, &limits, &workspace, &delete)),
        [("BRICK_ACCESS_MODE", Decision::Deny)]
    );
    let create = call(
        Operation::Write,
        &[],
        vec![change("app/new.py", false, false)],
        None,
        "claude",
    );
    assert_eq!(
        rules_of(&decide(&rules, &limits, &workspace, &create)),
        [("BRICK_ACCESS_MODE", Decision::RequireApproval)]
    );
    // Approval: a kind of change, wherever access allows it
    let manifest = call(
        Operation::Write,
        &[],
        vec![change("app/package.json", true, false)],
        None,
        "claude",
    );
    assert_eq!(
        rules_of(&decide(&rules, &limits, &workspace, &manifest)),
        [("BRICK_APPROVAL", Decision::RequireApproval)]
    );
    // A push names no file: the approval and the direct operation apply repository-wide
    let push = call(
        Operation::Execute,
        &[],
        Vec::new(),
        Some("git push origin main"),
        "claude",
    );
    assert_eq!(
        rules_of(&decide(&rules, &limits, &workspace, &push)),
        [
            ("BRICK_APPROVAL", Decision::RequireApproval),
            ("BRICK_ACTION_DIRECT", Decision::Deny)
        ]
    );
    // Hidden variables and network reach apply where the session works
    let worked = Usage {
        session: "s".into(),
        targets: std::collections::BTreeMap::from([(
            "zone:pay".to_string(),
            TargetUsage::default(),
        )]),
        ..Usage::default()
    };
    let mut reveal = call(
        Operation::Execute,
        &[],
        Vec::new(),
        Some("echo $AWS_SECRET_ACCESS_KEY"),
        "claude",
    );
    assert!(
        decide(&rules, &limits, &workspace, &reveal)
            .findings
            .is_empty(),
        "the session never worked in zone:pay"
    );
    reveal.usage = Some(&worked);
    assert_eq!(
        rules_of(&decide(&rules, &limits, &workspace, &reveal)),
        [("BRICK_HIDDEN_ENV", Decision::Deny)]
    );
    let hosts = vec!["https://evil.example.org/upload".to_string()];
    let mut fetch = call(Operation::Other, &[], Vec::new(), None, "claude");
    fetch.network = &hosts;
    fetch.usage = Some(&worked);
    assert_eq!(
        rules_of(&decide(&rules, &limits, &workspace, &fetch)),
        [("BRICK_NETWORK", Decision::Deny)]
    );
    let allowed = vec!["https://api.example.com/v1".to_string()];
    fetch.network = &allowed;
    assert!(decide(&rules, &limits, &workspace, &fetch)
        .findings
        .is_empty());
    // Limits: the third call touching zone:pay in one session stops
    let busy = Usage {
        session: "s".into(),
        targets: std::collections::BTreeMap::from([(
            "zone:pay".to_string(),
            TargetUsage {
                calls: 2,
                ..TargetUsage::default()
            },
        )]),
        ..Usage::default()
    };
    let mut third = call(Operation::Read, &["app/pay.py"], Vec::new(), None, "claude");
    third.usage = Some(&busy);
    let decided = decide(&rules, &limits, &workspace, &third);
    assert_eq!(rules_of(&decided), [("BRICK_LIMIT", Decision::Deny)]);
    assert!(decided
        .findings
        .iter()
        .all(|finding| finding.decision != Decision::Allow));
    assert_eq!(
        decided.counted.targets,
        BTreeSet::from(["zone:pay".to_string()])
    );
}

/** Former kinds are read as the current ones, and a share that does not resolve fails closed
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn former_kinds_and_shares() {
    let kinds = |kind: BrickType, value: serde_json::Value| {
        config::parse(kind, &value)
            .unwrap()
            .iter()
            .map(|config| serde_json::to_value(config).unwrap())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        kinds(BrickType::Scope, json!({"include": ["app/**"]}))[0]["include"],
        json!(["app/**"])
    );
    assert_eq!(
        kinds(BrickType::Constraint, json!({"protected_paths": ["db/**"]}))[0]["read_only"],
        json!(["db/**"])
    );
    assert_eq!(
        kinds(
            BrickType::Monitoring,
            json!({"signals": [{"source": "policy_tests", "every_minutes": 30}]})
        )[0]["signals"][0]["source"],
        json!("policy_tests")
    );
    assert!(matches!(
        config::parse(BrickType::Isolation, &json!({"profile": "agent-hooks"})).as_deref(),
        Ok([Config::Environment(_)])
    ));
    let directory = tempfile::tempdir().unwrap();
    let mut store = BrickStore::default();
    store.bricks.push(entry(
        brick(
            "count",
            1,
            BrickType::Limits,
            json!({"max_files_changed": 3, "scope_from": "edits"}),
            "",
        ),
        VersionState::Active,
    ));
    store.attachments = vec![attached("a1", "zone:z", "count@1", "")];
    let alone = controls(&store, "zone:z", directory.path());
    assert!(
        alone
            .fail_closed
            .iter()
            .any(|reason| reason.contains("SHARE_UNRESOLVED")),
        "{:?}",
        alone.fail_closed
    );
    store.bricks.push(entry(
        brick(
            "edits",
            1,
            BrickType::Access,
            json!({"include": ["app/**"], "exclude": ["app/generated/**"]}),
            "",
        ),
        VersionState::Active,
    ));
    store
        .attachments
        .push(attached("a2", "zone:z", "edits@1", ""));
    let shared = controls(&store, "zone:z", directory.path());
    assert!(shared.fail_closed.is_empty());
    assert!(
        shared.caps[0].paths.contains("app/pay.py")
            && !shared.caps[0].paths.contains("app/generated/api.py")
    );
}

/** Kinds of change, gates in time, and changed lines are recognized as documented
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn kinds_of_change_and_gates() {
    use super::config::ChangeClass;
    use super::guard::{command_classes, file_classes, lines_changed, Change};
    let classes = |path: &str| {
        file_classes(&Change {
            path: path.into(),
            existed: true,
            deleted: false,
            lines: 1,
        })
    };
    assert!(classes("db/migrations/001_init.sql").contains(&ChangeClass::MigrationChange));
    assert!(classes("config/app.yaml").contains(&ChangeClass::ConfigChange));
    assert!(classes("Cargo.lock").contains(&ChangeClass::DependencyChange));
    assert!(classes("web/cart.test.ts").contains(&ChangeClass::TestChange));
    assert!(!classes("src/main.rs").contains(&ChangeClass::ConfigChange));
    let words = |text: &str| {
        crate::hooks::shell::words(text)
            .into_iter()
            .map(|word| word.to_ascii_lowercase())
            .collect::<Vec<_>>()
    };
    assert!(command_classes(&words("npm install left-pad")).contains(&ChangeClass::PackageInstall));
    assert!(command_classes(&words("git commit -m x && git push")).contains(&ChangeClass::GitPush));
    assert_eq!(lines_changed(Some("a\nb\nc\n"), Some("a\nB\nc\nd\n")), 3);
    // 2026-10-28 was a Wednesday; 10:00 UTC is 15:30 in UTC+5:30
    let wednesday_ten = 1_793_181_600_000;
    let days = |list: &[&str]| list.iter().map(|day| day.to_string()).collect::<Vec<_>>();
    assert!(super::runtime::in_window(
        wednesday_ten,
        &days(&["wed"]),
        "09:00",
        "17:00",
        0
    ));
    assert!(!super::runtime::in_window(
        wednesday_ten,
        &days(&["sat", "sun"]),
        "09:00",
        "17:00",
        0
    ));
    assert!(!super::runtime::in_window(
        wednesday_ten,
        &[],
        "16:00",
        "18:00",
        330
    ));
    assert!(super::runtime::in_window(
        wednesday_ten,
        &[],
        "15:00",
        "16:00",
        330
    ));
    assert!(
        super::runtime::in_window(wednesday_ten, &[], "22:00", "11:00", 0),
        "a window over midnight"
    );
}
