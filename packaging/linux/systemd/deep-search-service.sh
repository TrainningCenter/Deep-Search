#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: deep-search-service <status|is-active|is-enabled|start|stop|restart|enable|disable> [linux-username]
Read-only actions do not require root. start/stop/restart/enable/disable require root.
EOF
  exit 2
}

[[ $# -ge 1 && $# -le 2 ]] || usage
action="$1"
username="${2:-$(id -un)}"

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
  echo "Refusing to manage a root-owned Deep-Search instance." >&2
  exit 1
}

unit="deep-search@${username}.service"

case "$action" in
  status)
    exec systemctl --no-pager --full status "$unit"
    ;;
  is-active)
    exec systemctl is-active "$unit"
    ;;
  is-enabled)
    exec systemctl is-enabled "$unit"
    ;;
  start|stop|restart|enable|disable)
    [[ ${EUID} -eq 0 ]] || {
      echo "Run '$action' as root (for example, with sudo)." >&2
      exit 1
    }
    exec systemctl "$action" "$unit"
    ;;
  *)
    usage
    ;;
esac
