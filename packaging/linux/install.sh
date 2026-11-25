#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "Usage: sudo $0 <linux-username> [package-directory]" >&2
  exit 2
}

[[ ${EUID} -eq 0 ]] || { echo "Run this installer as root (for example, with sudo)." >&2; exit 1; }
[[ $# -ge 1 && $# -le 2 ]] || usage

username="$1"
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
  echo "Deep-Search must run under a non-root user account." >&2
  exit 1
}

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
package_dir="${2:-$(cd -- "$script_dir/../.." && pwd)}"
engine="$package_dir/deep-search"
desktop_app="$package_dir/deep-search-ui"
systemd_dir="$script_dir/systemd"

for required in "$engine" "$desktop_app" "$systemd_dir/install-service.sh" "$script_dir/deep-search.desktop"; do
  [[ -f "$required" ]] || {
    echo "Required package file is missing: $required" >&2
    exit 1
  }
done
[[ -x "$engine" && -x "$desktop_app" ]] || {
  echo "The engine and desktop application must be executable." >&2
  exit 1
}

install -D -m 0755 "$desktop_app" /usr/local/bin/deep-search-ui
install -D -m 0644 "$script_dir/deep-search.desktop" /usr/share/applications/deep-search.desktop

bash "$systemd_dir/install-service.sh" "$username" "$engine"

echo
echo "Deep-Search desktop application and background service are installed."
echo "Launch it from your desktop application menu, or run: deep-search-ui"
echo "The service is enabled at boot and runs as '$username'."
