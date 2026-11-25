#!/usr/bin/env bash
set -euo pipefail

helper="packaging/macos/launchd/deep-search-service.sh"
bash -n "$helper"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
cat > "$tmp/launchctl" <<'MOCK'
#!/usr/bin/env bash
printf '%s\n' "$*" > "$DEEP_SEARCH_LAUNCHCTL_ARGS"
if [[ "$1" == "print" && "${MOCK_LAUNCHCTL_INACTIVE:-0}" == "1" ]]; then
  exit 1
fi
echo "mock launchctl: $*"
MOCK
chmod +x "$tmp/launchctl"
export PATH="$tmp:$PATH"
export DEEP_SEARCH_LAUNCHCTL_ARGS="$tmp/launchctl-args"

output="$(bash "$helper" status)"
[[ "$output" == *"mock launchctl: print system/com.deepsearch.service"* ]]

output="$(bash "$helper" is-active)"
[[ "$output" == "active" ]]
export MOCK_LAUNCHCTL_INACTIVE=1
output="$(bash "$helper" is-active)"
[[ "$output" == "inactive" ]]
unset MOCK_LAUNCHCTL_INACTIVE

bash "$helper" start >/dev/null
[[ "$(cat "$DEEP_SEARCH_LAUNCHCTL_ARGS")" == "kickstart system/com.deepsearch.service" ]]

bash "$helper" stop >/dev/null
[[ "$(cat "$DEEP_SEARCH_LAUNCHCTL_ARGS")" == "kill SIGTERM system/com.deepsearch.service" ]]

bash "$helper" restart >/dev/null
[[ "$(cat "$DEEP_SEARCH_LAUNCHCTL_ARGS")" == "kickstart -k system/com.deepsearch.service" ]]

if bash "$helper" invalid >"$tmp/invalid.out" 2>&1; then
  echo "Expected invalid action to fail." >&2
  exit 1
fi
grep -q "Usage:" "$tmp/invalid.out"

echo "macOS launchd service helper checks passed."
