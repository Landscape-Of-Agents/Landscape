#!/usr/bin/env bash
# Smoke test of a built crane binary: connect a clone to its (local mirror) remote, initialize,
# checkpoint, protect a region, pass the policy tests, break the protected code, and fail them.
set -euo pipefail
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

"$BIN" --version | grep -Eq '^crane [0-9]+\.[0-9]+\.[0-9]+$'

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

"$BIN" repo --https "$DEMO/remote.git" rf | grep -q "CONNECTED"
"$BIN" repo status | grep -q "^CONNECTED"
"$BIN" init | grep -q "Initialized"
"$BIN" checkpoint baseline | grep -q "Created checkpoint 'baseline'"
"$BIN" create policy payment_gateway gateway | grep -q "Created policy payment_gateway"
"$BIN" protect GatewayService.java policy payment_gateway start-line 2 end-line 4 | grep -q "as selection"
grep -q "// @crane:selection:" GatewayService.java
"$BIN" validate | grep -q "Crane validate: PASS"
"$BIN" test . | grep -q "Crane test: PASS"
"$BIN" policy payment_gateway status >/dev/null

sed -i.bak 's/"payment"/"modified"/' GatewayService.java && rm -f GatewayService.java.bak
if "$BIN" test . >"$DEMO/fail.txt" 2>&1; then
  echo "expected crane test to fail after the protected code changed" >&2
  exit 1
fi
grep -q "FAIL policy payment_gateway" "$DEMO/fail.txt"
grep -q "Crane test: FAIL" "$DEMO/fail.txt"
echo "Smoke test passed."
