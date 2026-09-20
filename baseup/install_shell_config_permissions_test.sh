#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TEST_ROOT="$(mktemp -d)"
trap 'chmod -R u+w "$TEST_ROOT" 2>/dev/null || true; rm -rf "$TEST_ROOT"' EXIT

mkdir -p "$TEST_ROOT/mock-bin"
cat > "$TEST_ROOT/source-baseup" <<'SRC'
#!/usr/bin/env bash
: > "${BASEUP_TEST_MARKER:?}"
exit 0
SRC
chmod +x "$TEST_ROOT/source-baseup"

cat > "$TEST_ROOT/mock-bin/curl" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail

out=""
while (($#)); do
    case "$1" in
        -o)
            out="$2"
            shift 2
            ;;
        *)
            shift
            ;;
    esac
done

[[ -n "$out" ]]
cp "$FIXTURE_SOURCE" "$out"
MOCK
chmod +x "$TEST_ROOT/mock-bin/curl"

run_install() {
    local home="$1"
    local base_home="$2"
    local bin_dir="$3"
    local marker="$4"

    HOME="$home" \
        SHELL=/bin/bash \
        XDG_CONFIG_HOME="$home/.config" \
        PATH="$TEST_ROOT/mock-bin:$PATH" \
        FIXTURE_SOURCE="$TEST_ROOT/source-baseup" \
        BASEUP_TEST_MARKER="$marker" \
        BASEUP_HOME="$base_home" \
        BASE_BIN_DIR="$bin_dir" \
        BASEUP_URL=https://example.invalid/baseup \
        bash "$SCRIPT_DIR/install"
}

readonly_home="$TEST_ROOT/readonly-home"
readonly_base_home="$TEST_ROOT/readonly-base-home"
readonly_bin="$TEST_ROOT/readonly-bin"
readonly_marker="$TEST_ROOT/readonly-ran"
mkdir -p "$readonly_home/.config/fish" "$readonly_base_home" "$readonly_bin"

printf 'readonly bash content\n' > "$readonly_home/.bashrc"
printf 'readonly fish content\n' > "$readonly_home/.config/fish/config.fish"
cp "$readonly_home/.bashrc" "$TEST_ROOT/bashrc.before"
cp "$readonly_home/.config/fish/config.fish" "$TEST_ROOT/fish.before"
chmod 0444 "$readonly_home/.bashrc" "$readonly_home/.config/fish/config.fish"

run_install "$readonly_home" "$readonly_base_home" "$readonly_bin" "$readonly_marker" \
    > "$TEST_ROOT/readonly.log" 2>&1

cmp -s "$TEST_ROOT/bashrc.before" "$readonly_home/.bashrc"
cmp -s "$TEST_ROOT/fish.before" "$readonly_home/.config/fish/config.fish"
[[ -f "$readonly_marker" ]]
[[ -x "$readonly_bin/baseup" ]]
grep -qF "shell config $readonly_home/.bashrc is not writable; leaving it unchanged" "$TEST_ROOT/readonly.log"
grep -qF "shell config $readonly_home/.config/fish/config.fish is not writable; leaving it unchanged" "$TEST_ROOT/readonly.log"

writable_home="$TEST_ROOT/writable-home"
writable_base_home="$TEST_ROOT/writable-base-home"
writable_bin="$TEST_ROOT/writable-bin"
writable_marker="$TEST_ROOT/writable-ran"
mkdir -p "$writable_home" "$writable_base_home" "$writable_bin"
printf 'user content\n' > "$writable_home/.bashrc"

run_install "$writable_home" "$writable_base_home" "$writable_bin" "$writable_marker" \
    > "$TEST_ROOT/writable.log" 2>&1

[[ -f "$writable_marker" ]]
grep -qF ". \"$writable_base_home/env\"" "$writable_home/.bashrc"
[[ "$(grep -cF ". \"$writable_base_home/env\"" "$writable_home/.bashrc")" -eq 1 ]]
grep -qF 'user content' "$writable_home/.bashrc"

echo "baseup shell config permission test passed"
