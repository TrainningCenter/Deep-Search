#!/usr/bin/env bash
set -euo pipefail
label="com.deepsearch.service"
domain="system/$label"
plist="/Library/LaunchDaemons/$label.plist"
action="${1:-}"
case "$action" in
  status)
    exec launchctl print "$domain"
    ;;
  is-active)
    if launchctl print "$domain" >/dev/null 2>&1; then echo active; else echo inactive; fi
    ;;
  is-enabled)
    if [[ -f "$plist" ]] && launchctl print-disabled system 2>/dev/null | grep -F "$label" | grep -q 'enabled'; then
      echo enabled
    elif [[ -f "$plist" ]]; then
      echo disabled
    else
      echo not-found
      exit 1
    fi
    ;;
  start)
    exec launchctl kickstart "$domain"
    ;;
  stop)
    exec launchctl kill SIGTERM "$domain"
    ;;
  restart)
    exec launchctl kickstart -k "$domain"
    ;;
  enable)
    [[ ${EUID} -eq 0 ]] || { echo "Run enable as root." >&2; exit 1; }
    launchctl enable "$domain"
    if ! launchctl print "$domain" >/dev/null 2>&1 && [[ -f "$plist" ]]; then
      launchctl bootstrap system "$plist"
    fi
    ;;
  disable)
    [[ ${EUID} -eq 0 ]] || { echo "Run disable as root." >&2; exit 1; }
    launchctl disable "$domain"
    launchctl bootout system "$plist" >/dev/null 2>&1 || true
    ;;
  *)
    echo "Usage: deep-search-service <status|is-active|is-enabled|start|stop|restart|enable|disable>" >&2
    exit 2
    ;;
esac
