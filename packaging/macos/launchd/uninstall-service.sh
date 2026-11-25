#!/usr/bin/env bash
set -euo pipefail
[[ ${EUID} -eq 0 ]] || { echo "Run this uninstaller as root." >&2; exit 1; }
label="com.deepsearch.service"
plist="/Library/LaunchDaemons/$label.plist"
launchctl disable "system/$label" >/dev/null 2>&1 || true
launchctl bootout system "$plist" >/dev/null 2>&1 || true
rm -f "$plist" /usr/local/bin/deep-search-service
rm -rf /Applications/Deep-Search.app
echo "Deep-Search launchd service removed. The index database and engine binary were preserved."
