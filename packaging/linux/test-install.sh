#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
work="$(mktemp -d)"
trap 'chmod -R u+rwX "$work" 2>/dev/null || true; rm -rf "$work"' EXIT
package="$work/package"
mkdir -p "$package/packaging/linux/systemd" "$work/mockbin" "$work/prefix"

cp "$script_dir/install.sh" "$package/packaging/linux/install.sh"
cp "$script_dir/deep-search.desktop" "$package/packaging/linux/deep-search.desktop"
printf '#!/bin/sh\nexit 0\n' > "$package/deep-search"
printf '#!/bin/sh\nexit 0\n' > "$package/deep-search-ui"
chmod 0755 "$package/deep-search" "$package/deep-search-ui"

cat > "$package/packaging/linux/systemd/install-service.sh" <<'MOCK_SERVICE'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n%s\n' "$1" "$2" > "$TEST_INSTALL_LOG"
MOCK_SERVICE

cat > "$work/mockbin/install" <<'MOCK_INSTALL'
#!/usr/bin/env bash
set -euo pipefail
args=()
while (($#)); do
  case "$1" in
    -D) shift ;;
    -m) shift 2 ;;
    *) args+=("$1"); shift ;;
  esac
done
source_file="${args[0]}"
destination="${args[1]}"
mkdir -p "$TEST_PREFIX$(dirname "$destination")"
cp "$source_file" "$TEST_PREFIX$destination"
MOCK_INSTALL
chmod 0755 "$work/mockbin/install"

export TEST_PREFIX="$work/prefix"
export TEST_INSTALL_LOG="$work/service.log"
export PATH="$work/mockbin:$PATH"

if [[ ${EUID} -eq 0 ]]; then
  bash "$package/packaging/linux/install.sh" nobody "$package"
else
  sudo --preserve-env=PATH,TEST_PREFIX,TEST_INSTALL_LOG \
    bash "$package/packaging/linux/install.sh" nobody "$package"
fi

grep -qx 'nobody' "$TEST_INSTALL_LOG"
grep -qx "$package/deep-search" <(sed -n '2p' "$TEST_INSTALL_LOG")
test -x "$TEST_PREFIX/usr/local/bin/deep-search-ui"
test -f "$TEST_PREFIX/usr/share/applications/deep-search.desktop"

echo "Linux desktop installer smoke test passed."
