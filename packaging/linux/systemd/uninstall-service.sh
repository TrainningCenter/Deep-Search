#!/usr/bin/env bash
set -euo pipefail

[[ ${EUID} -eq 0 ]] || { echo "Run this script as root (for example, with sudo)." >&2; exit 1; }
[[ $# -eq 1 ]] || {
  echo "Usage: sudo $0 <linux-username>" >&2
  exit 2
}
username="$1"
case "$username" in
  ""|*[!a-zA-Z0-9_.-]*)
    echo "Invalid Linux username." >&2
    exit 2
    ;;
esac

systemctl disable --now "deep-search@${username}.service"
echo "Disabled Deep-Search for '$username'. The user's index database was preserved."
