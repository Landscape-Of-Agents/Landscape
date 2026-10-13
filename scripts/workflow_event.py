#!/usr/bin/env python3
"""Print one event of a GitHub workflow (DASHBOARD.md F32 pull request notifications and F33
building, releasing, and publishing) as a JSON line in the workflow log, prefixed with
"crane-event ". Workflows run outside Crane's runtime, so these events are not uploaded; the log
line is their record.

Usage: workflow_event.py EVENT_NAME FLOW group.key=value ...
A value is read as JSON when it parses (numbers, true/false, null, ["lists"]), else as text.
Every group gets its compulsory defaults for a workflow; the given values override them.
"""

import hashlib
import json
import os
import sys
import time
import uuid

SPEC_VERSION = 1
GROUPS = ["action", "repository", "session", "agent", "resource", "response", "assessment",
          "decision", "eval", "severity", "bindings", "telemetry", "other"]


def repository_kind():
    """PERSONAL or ORGANIZATION, from the repository's owner in the workflow's event payload."""
    try:
        with open(os.environ.get("GITHUB_EVENT_PATH", ""), encoding="utf-8") as handle:
            owner = json.load(handle).get("repository", {}).get("owner", {})
        return "ORGANIZATION" if owner.get("type") == "Organization" else "PERSONAL"
    except (OSError, ValueError, AttributeError):
        return "PERSONAL"


def defaults(name, flow):
    """The event with every compulsory sub-attribute set for a workflow."""
    return {
        "event_id": "evt_" + uuid.uuid4().hex,
        "event_name": name,
        "flow": flow,
        "occurred_at": int(time.time() * 1000),
        "schema_version": SPEC_VERSION,
        "sequence": None,
        "previous_digest": None,
        "origin": "WORKFLOW",
        "action": {"action_type": "CI", "action_source": "CI",
                   "action_user": os.environ.get("GITHUB_ACTOR"), "action_email": None,
                   "action_id": os.environ.get("GITHUB_RUN_ID")},
        "repository": {"repo": repository_kind(), "repo_id": None,
                       "repo_remote": "github.com/" + os.environ.get("GITHUB_REPOSITORY", ""),
                       "repo_remote_verified": False, "registry_generation": None},
        "session": {"session_type": "NOT_IN_SESSION", "session_id": None,
                    "provider_session_id": None, "id_provenance": "NA", "task_id": None,
                    "external_task_id": None},
        "agent": {"agent_type": "NOT_AN_AGENT", "agent_id": None, "model": None,
                  "autonomy_mode": "NA"},
        "resource": {"resource": ["GIT"], "resource_ids": [os.environ.get("GITHUB_SHA")],
                     "resource_classification": ["NONE"]},
        "response": {"response_status": "SUCCESS", "failure_state": "NA",
                     "response_statement": ""},
        "assessment": {"assessment": "NOT_REQUIRED", "assessment_checks": []},
        "decision": {"decision": "NOT_REQUIRED", "decision_rule": None, "decision_reasons": [],
                     "enforcement": "NOT_APPLICABLE"},
        "eval": {"eval": "NOT_REQUIRED"},
        "severity": {"severity_status": "INFO", "severity_basis": "OUTCOME",
                     "alert_kind": "NONE"},
        "bindings": {"third_party": ["GITHUB_ACTIONS"], "binding_result": "OK"},
        "telemetry": {"agent_state": "INACTIVE", "known_as": "OBSERVED", "hook_event": "NA"},
        "other": {},
    }


def parse(text):
    """A value as JSON when it parses, else as text."""
    try:
        return json.loads(text)
    except ValueError:
        return text


def main(arguments):
    if len(arguments) < 2:
        print(__doc__, file=sys.stderr)
        return 2
    event = defaults(arguments[0], arguments[1])
    for pair in arguments[2:]:
        key, separator, value = pair.partition("=")
        group, dot, name = key.partition(".")
        if not separator or not dot or group not in GROUPS or not name:
            print(f"'{pair}' is not group.key=value", file=sys.stderr)
            return 2
        event[group][name] = parse(value)
        if group == "eval" and name != "eval":
            event["eval"]["eval"] = "MEASURED"
    # At most ten words, as on every other event
    statement = event["response"]["response_statement"] or ""
    event["response"]["response_statement"] = " ".join(str(statement).split()[:10])
    canonical = json.dumps(event, sort_keys=True, separators=(",", ":"))
    event["digest"] = hashlib.sha512(canonical.encode("utf-8")).hexdigest()
    print("crane-event " + json.dumps(event, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
