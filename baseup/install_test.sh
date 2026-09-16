#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TEST_ROOT="$(mktemp -d)"
trap 'rm -rf "$TEST_ROOT"' EXIT

mkdir -p "$TEST_ROOT/mock-bin" "$TEST_ROOT/home"

printf '%s\n' \
    '#!/usr/bin/env bash' \
    'set -euo pipefail' \
    'output=""' \
    'while [[ $# -gt 0 ]]; do' \
    '    if [[ "$1" == "-o" ]]; then' \
    '        output="$2"' \
    '        shift 2' \
    '    else' \
    '        shift' \
    '    fi' \
    'done' \
    'printf '\''%s\n'\'' '\''#!/usr/bin/env bash'\'' '\''exit 0'\'' > "$output"' \
    > "$TEST_ROOT/mock-bin/curl"
chmod +x "$TEST_ROOT/mock-bin/curl"

run_install() {
    local bin_dir="$1"

    HOME="$TEST_ROOT/home" \
        SHELL=/bin/false \
        PATH="$TEST_ROOT/mock-bin:$PATH" \
        BASEUP_HOME="$TEST_ROOT/base-home" \
        BASE_BIN_DIR="$bin_dir" \
        BASEUP_URL=https://example.invalid/baseup \
        bash "$SCRIPT_DIR/install" >/dev/null
}

BIN_A="$TEST_ROOT/bin-a"
BIN_B="$TEST_ROOT/bin-b"
ENV_FILE="$TEST_ROOT/base-home/env"

run_install "$BIN_A"
printf '%s\n' '# user shell customization' 'export BASE_TEST_VALUE=kept' >> "$ENV_FILE"
printf '%s\n' '# user fish customization' 'set -gx BASE_TEST_VALUE kept' >> "$ENV_FILE.fish"
run_install "$BIN_B"

test -x "$BIN_B/baseup"
grep -qxF "export PATH=\"$BIN_B:\$PATH\"" "$ENV_FILE"
grep -qxF "fish_add_path -g \"$BIN_B\"" "$ENV_FILE.fish"
! grep -qF "$BIN_A" "$ENV_FILE"
! grep -qF "$BIN_A" "$ENV_FILE.fish"
grep -qxF 'export BASE_TEST_VALUE=kept' "$ENV_FILE"
grep -qxF 'set -gx BASE_TEST_VALUE kept' "$ENV_FILE.fish"

printf 'baseup bootstrap environment tests passed\n'
