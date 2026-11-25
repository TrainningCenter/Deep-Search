#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "Usage: sudo $0 <linux-username> <path-to-deep-search-binary>" >&2
  exit 2
}

[[ ${EUID} -eq 0 ]] || { echo "Run this installer as root (for example, with sudo)." >&2; exit 1; }
[[ $# -eq 2 ]] || usage

username="$1"
binary_path="$2"

case "$username" in
  ""|*[!a-zA-Z0-9_.-]*)
    echo "Invalid Linux username." >&2
    exit 2
    ;;
esac

getent passwd "$username" >/dev/null || {
  echo "Linux user '$username' does not exist." >&2
  exit 1
}
[[ "$(id -u "$username")" -ne 0 ]] || {
  echo "Refusing to run the background engine as root." >&2
  exit 1
}
[[ -f "$binary_path" && -x "$binary_path" ]] || {
  echo "Binary not found or not executable: $binary_path" >&2
  exit 1
}

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
install -D -m 0755 "$binary_path" /usr/local/bin/deep-search
install -D -m 0644 "$script_dir/deep-search@.service" /etc/systemd/system/deep-search@.service

systemctl daemon-reload
systemctl enable --now "deep-search@${username}.service"
systemctl --no-pager --full status "deep-search@${username}.service"
