#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TEST_ROOT="$(mktemp -d)"
trap 'rm -rf "$TEST_ROOT"' EXIT

mkdir -p "$TEST_ROOT/mock-bin" "$TEST_ROOT/home" "$TEST_ROOT/bin/baseup"

cat > "$TEST_ROOT/source" <<'SRC'
#!/usr/bin/env bash
exit 0
SRC
chmod +x "$TEST_ROOT/source"

export FIXTURE_ROOT="$TEST_ROOT"
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
cp "$FIXTURE_ROOT/source" "$out"
MOCK
chmod +x "$TEST_ROOT/mock-bin/curl"

set +e
HOME="$TEST_ROOT/home" \
SHELL=/bin/false \
PATH="$TEST_ROOT/mock-bin:$PATH" \
BASEUP_HOME="$TEST_ROOT/base-home" \
BASE_BIN_DIR="$TEST_ROOT/bin" \
BASEUP_URL="https://example.invalid/baseup" \
bash "$SCRIPT_DIR/install" > "$TEST_ROOT/install.log" 2>&1
status=$?
set -e

if (( status == 0 )); then
    cat "$TEST_ROOT/install.log" >&2
    echo "installer unexpectedly accepted a directory at the baseup destination" >&2
    exit 1
fi

if ! grep -Fq "destination $TEST_ROOT/bin/baseup is a directory" "$TEST_ROOT/install.log"; then
    cat "$TEST_ROOT/install.log" >&2
    echo "installer did not report the directory-destination error" >&2
    exit 1
fi

if [[ ! -d "$TEST_ROOT/bin/baseup" ]]; then
    echo "existing destination directory was modified" >&2
    exit 1
fi

if find "$TEST_ROOT/bin/baseup" -mindepth 1 -print -quit | grep -q .; then
    echo "installer wrote into the existing destination directory" >&2
    exit 1
fi

if [[ -e "$TEST_ROOT/bin/baseup.new" ]]; then
    echo "installer left a temporary destination behind" >&2
    exit 1
fi

echo "baseup directory destination test passed"
