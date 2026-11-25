#!/usr/bin/env bash
set -euo pipefail

script="packaging/linux/systemd/deep-search-service.sh"
bash -n "$script"

output="$(bash "$script" 2>&1)" && {
  echo "Expected missing-argument invocation to fail." >&2
  exit 1
}
[[ "$output" == *"Usage:"* ]]

output="$(bash "$script" invalid 2>&1)" && {
  echo "Expected invalid action to fail." >&2
  exit 1
}
[[ "$output" == *"Usage:"* ]]

output="$(bash "$script" status 'bad;user' 2>&1)" && {
  echo "Expected invalid username to fail." >&2
  exit 1
}
[[ "$output" == *"Invalid Linux username."* ]]

echo "Service control script checks passed."
