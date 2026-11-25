#!/usr/bin/env bash
set -euo pipefail

script="packaging/linux/systemd/deep-search-service.sh"
bash -n "$script"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
cat > "$tmp/getent" <<'MOCK'
#!/usr/bin/env bash
[[ "$#" -eq 2 && "$1" == "passwd" && "$2" == "testuser" ]]
MOCK
cat > "$tmp/id" <<'MOCK'
#!/usr/bin/env bash
if [[ "$1" == "-u" && "$2" == "testuser" ]]; then
  echo 1000
else
  exit 1
fi
MOCK
cat > "$tmp/systemctl" <<'MOCK'
#!/usr/bin/env bash
printf '%s\n' "$*" > "$DEEP_SEARCH_SYSTEMCTL_ARGS"
MOCK
cat > "$tmp/journalctl" <<'MOCK'
#!/usr/bin/env bash
printf '%s\n' "$*" > "$DEEP_SEARCH_JOURNAL_ARGS"
MOCK
chmod +x "$tmp/getent" "$tmp/id" "$tmp/systemctl" "$tmp/journalctl"
export PATH="$tmp:$PATH"
export DEEP_SEARCH_SYSTEMCTL_ARGS="$tmp/systemctl-args"
export DEEP_SEARCH_JOURNAL_ARGS="$tmp/journal-args"

output="$(bash "$script" 2>&1)" && {
  echo "Expected missing-argument invocation to fail." >&2
  exit 1
}
[[ "$output" == *"Usage:"* ]]

output="$(bash "$script" invalid testuser 2>&1)" && {
  echo "Expected invalid action to fail." >&2
  exit 1
}
[[ "$output" == *"Usage:"* ]]

output="$(bash "$script" status 'bad;user' 2>&1)" && {
  echo "Expected invalid username to fail." >&2
  exit 1
}
[[ "$output" == *"Invalid Linux username."* ]]

bash "$script" is-active testuser
[[ "$(cat "$DEEP_SEARCH_SYSTEMCTL_ARGS")" == "is-active deep-search@testuser.service" ]]

bash "$script" logs testuser
[[ "$(cat "$DEEP_SEARCH_JOURNAL_ARGS")" == "--unit deep-search@testuser.service --no-pager --lines=100" ]]

bash "$script" logs-follow testuser
[[ "$(cat "$DEEP_SEARCH_JOURNAL_ARGS")" == "--unit deep-search@testuser.service --follow" ]]

if bash "$script" start testuser >"$tmp/start.out" 2>&1; then
  echo "Expected non-root start request to be rejected." >&2
  exit 1
fi
grep -q "Run 'start' as root" "$tmp/start.out"

echo "Service control helper checks passed."
