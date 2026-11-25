#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "Usage: sudo $0 <macOS-username> <path-to-deep-search-binary>" >&2
  exit 2
}

[[ ${EUID} -eq 0 ]] || { echo "Run this installer as root (for example, with sudo)." >&2; exit 1; }
[[ $# -eq 2 ]] || usage
username="$1"
binary_path="$2"

case "$username" in
  ""|*[!a-zA-Z0-9_.-]*) echo "Invalid macOS username." >&2; exit 2 ;;
esac

user_record="$(dscl . -read "/Users/$username" NFSHomeDirectory 2>/dev/null)" || {
  echo "macOS user '$username' does not exist." >&2
  exit 1
}
home_dir="${user_record#NFSHomeDirectory: }"
uid="$(id -u "$username")"
[[ "$uid" -ne 0 ]] || { echo "Refusing to run Deep-Search as root." >&2; exit 1; }
[[ -f "$binary_path" && -x "$binary_path" ]] || {
  echo "Binary not found or not executable: $binary_path" >&2
  exit 1
}

xml_escape() {
  printf '%s' "$1" | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g'
}

home_xml="$(xml_escape "$home_dir")"
db_dir="$home_dir/Library/Application Support/Deep-Search"
log_dir="$home_dir/Library/Logs/Deep-Search"
db_path="$db_dir/index.sqlite"
mkdir -p "$db_dir" "$log_dir" /usr/local/bin /Library/LaunchDaemons
chown -R "$username" "$home_dir/Library/Application Support/Deep-Search" "$log_dir"
install -m 0755 "$binary_path" /usr/local/bin/deep-search
install -m 0755 "$(dirname -- "${BASH_SOURCE[0]}")/deep-search-service.sh" /usr/local/bin/deep-search-service

plist="/Library/LaunchDaemons/com.deepsearch.service.plist"
cat > "$plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.deepsearch.service</string>
  <key>ProgramArguments</key>
  <array>
    <string>/usr/local/bin/deep-search</string>
    <string>serve</string>
    <string>$(xml_escape "$db_path")</string>
    <string>127.0.0.1:4765</string>
  </array>
  <key>UserName</key><string>$(xml_escape "$username")</string>
  <key>WorkingDirectory</key><string>$home_xml</string>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>Umask</key><integer>63</integer>
  <key>ProcessType</key><string>Background</string>
  <key>StandardOutPath</key><string>$(xml_escape "$log_dir/service.log")</string>
  <key>StandardErrorPath</key><string>$(xml_escape "$log_dir/service-error.log")</string>
</dict>
</plist>
EOF
chown root:wheel "$plist"
chmod 0644 "$plist"

launchctl bootout system "$plist" >/dev/null 2>&1 || true
launchctl enable system/com.deepsearch.service
launchctl bootstrap system "$plist"
launchctl kickstart -k system/com.deepsearch.service
echo "Deep-Search installed for $username and configured to start at boot before login."
