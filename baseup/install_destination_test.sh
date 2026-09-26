#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TEST_ROOT="$(mktemp -d)"
trap 'chmod u+w "$TEST_ROOT/blocked-bin" 2>/dev/null || true; rm -rf "$TEST_ROOT"' EXIT

mkdir -p "$TEST_ROOT/mock-bin" "$TEST_ROOT/home" "$TEST_ROOT/base-home" "$TEST_ROOT/bin"

cat > "$TEST_ROOT/source-baseup" <<'SRC'
#!/usr/bin/env bash
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
    local bin_dir="$1"
    HOME="$TEST_ROOT/home" \
        SHELL=/bin/false \
        PATH="$TEST_ROOT/mock-bin:$PATH" \
        FIXTURE_SOURCE="$TEST_ROOT/source-baseup" \
        BASEUP_HOME="$TEST_ROOT/base-home" \
        BASE_BIN_DIR="$bin_dir" \
        BASEUP_URL=https://example.invalid/baseup \
        bash "$SCRIPT_DIR/install"
}

printf '#!/usr/bin/env bash\nexit 99\n' > "$TEST_ROOT/bin/baseup"
chmod 0555 "$TEST_ROOT/bin/baseup"

run_install "$TEST_ROOT/bin" > "$TEST_ROOT/replace.log" 2>&1

if ! cmp -s "$TEST_ROOT/source-baseup" "$TEST_ROOT/bin/baseup"; then
    cat "$TEST_ROOT/replace.log" >&2
    echo "read-only existing baseup was not replaced" >&2
    exit 1
fi

if [[ ! -x "$TEST_ROOT/bin/baseup" ]]; then
    echo "replacement baseup is not executable" >&2
    exit 1
fi

mkdir -p "$TEST_ROOT/blocked-bin"
chmod 0555 "$TEST_ROOT/blocked-bin"

set +e
run_install "$TEST_ROOT/blocked-bin" > "$TEST_ROOT/blocked.log" 2>&1
blocked_status=$?
set -e

if (( blocked_status == 0 )); then
    cat "$TEST_ROOT/blocked.log" >&2
    echo "installer unexpectedly accepted a non-writable install directory" >&2
    exit 1
fi

if ! grep -q "destination .* is not writable" "$TEST_ROOT/blocked.log"; then
    cat "$TEST_ROOT/blocked.log" >&2
    echo "non-writable directory did not report the expected error" >&2
    exit 1
fi

echo "baseup destination permission test passed"
