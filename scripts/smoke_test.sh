#!/usr/bin/env bash
# Smoke test of a built crane binary: connect a clone to its (local mirror) remote, initialize,
# checkpoint, protect a region, pass the policy tests, break the protected code, and fail them.
set -euo pipefail
# Checks read crane's whole output (grep > /dev/null, not grep -q): grep -q exits at the first
# match, and crane would then write into a closed pipe
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="${CRANE_BIN:-$ROOT/target/debug/crane}"
if [[ ! -x "$BIN" ]]; then
  echo "Crane binary is not executable: $BIN" >&2
  exit 1
fi
DEMO="$(mktemp -d)"
trap 'rm -rf "$DEMO"' EXIT
unset CLAUDECODE CLAUDE_CODE_ENTRYPOINT CODEX_SANDBOX CODEX_SANDBOX_NETWORK_DISABLED CRANE_AGENT
export CRANE_HOME="$DEMO/trust" CRANE_ALLOW_LOCAL_REMOTE=1 CRANE_ALERTS_DRY_RUN=1
# Never upload smoke-test data to the production ingestion service
export CRANE_INGEST_URL="${CRANE_INGEST_URL:-off}"

"$BIN" --version | grep -E '^crane [0-9]+\.[0-9]+\.[0-9]+$' > /dev/null

git init -q --bare -b main "$DEMO/remote.git"
git clone -q "$DEMO/remote.git" "$DEMO/work" 2>/dev/null
cd "$DEMO/work"
git config user.email crane@example.com
git config user.name "Crane Demo"
git config core.autocrlf false
cat > GatewayService.java <<'JAVA'
class GatewayService {
    public void call() {
        System.out.println("payment");
    }
}
JAVA
git add GatewayService.java
git commit -qm "trusted baseline"
git push -q origin HEAD:main

"$BIN" repo --https "$DEMO/remote.git" rf | grep "CONNECTED" > /dev/null
"$BIN" repo status | grep "^CONNECTED" > /dev/null
"$BIN" init | grep "Initialized" > /dev/null
"$BIN" checkpoint baseline | grep "Created checkpoint 'baseline'" > /dev/null
"$BIN" create policy payment_gateway gateway | grep "Created policy payment_gateway" > /dev/null
"$BIN" protect GatewayService.java policy payment_gateway start-line 2 end-line 4 | grep "as selection" > /dev/null
grep -q "// @crane:selection:" GatewayService.java
"$BIN" validate | grep "Crane validate: PASS" > /dev/null
"$BIN" test . | grep "Crane test: PASS" > /dev/null
"$BIN" policy payment_gateway status >/dev/null

sed -i.bak 's/"payment"/"modified"/' GatewayService.java && rm -f GatewayService.java.bak
if "$BIN" test . >"$DEMO/fail.txt" 2>&1; then
  echo "expected crane test to fail after the protected code changed" >&2
  exit 1
fi
grep -q "FAIL policy payment_gateway" "$DEMO/fail.txt"
grep -q "Crane test: FAIL" "$DEMO/fail.txt"
echo "Smoke test passed."
