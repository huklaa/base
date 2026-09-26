#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TEST_ROOT="$(mktemp -d)"
trap 'rm -rf "$TEST_ROOT"' EXIT

mkdir -p "$TEST_ROOT/mock-bin" "$TEST_ROOT/home" "$TEST_ROOT/barrier"

export FIXTURE_ROOT="$TEST_ROOT"
export REAL_CP="$(command -v cp)"

cat > "$TEST_ROOT/source-a" <<'SRC'
#!/usr/bin/env bash
# source-a
exit 0
SRC

cat > "$TEST_ROOT/source-b" <<'SRC'
#!/usr/bin/env bash
# source-b
exit 0
SRC

chmod +x "$TEST_ROOT/source-a" "$TEST_ROOT/source-b"

cat > "$TEST_ROOT/mock-bin/curl" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail

out=""
url=""
while (($#)); do
    case "$1" in
        -o)
            out="$2"
            shift 2
            ;;
        http://*|https://*)
            url="$1"
            shift
            ;;
        *)
            shift
            ;;
    esac
done

[[ -n "$out" && -n "$url" ]]
case "$url" in
    */a) cat "$FIXTURE_ROOT/source-a" > "$out" ;;
    */b) cat "$FIXTURE_ROOT/source-b" > "$out" ;;
    *) exit 91 ;;
esac
MOCK

cat > "$TEST_ROOT/mock-bin/cp" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail

"$REAL_CP" "$@"
dst="${!#}"

if [[ "$dst" == "$FIXTURE_ROOT/bin/baseup.new"* ]]; then
    touch "$FIXTURE_ROOT/barrier/cp.$$"

    for ((attempt = 0; attempt < 500; attempt++)); do
        count="$(find "$FIXTURE_ROOT/barrier" -maxdepth 1 -name 'cp.*' -type f | wc -l)"
        if [[ "$count" -ge 2 ]]; then
            exit 0
        fi
        sleep 0.01
    done

    exit 92
fi
MOCK

chmod +x "$TEST_ROOT/mock-bin/curl" "$TEST_ROOT/mock-bin/cp"

run_install() {
    local suffix="$1"

    HOME="$TEST_ROOT/home" \
        SHELL=/bin/false \
        PATH="$TEST_ROOT/mock-bin:$PATH" \
        BASEUP_HOME="$TEST_ROOT/base-home" \
        BASE_BIN_DIR="$TEST_ROOT/bin" \
        BASEUP_URL="https://example.invalid/$suffix" \
        bash "$SCRIPT_DIR/install"
}

set +e
run_install a > "$TEST_ROOT/a.log" 2>&1 &
pid_a=$!
run_install b > "$TEST_ROOT/b.log" 2>&1 &
pid_b=$!

wait "$pid_a"
status_a=$?
wait "$pid_b"
status_b=$?
set -e

if (( status_a != 0 || status_b != 0 )); then
    cat "$TEST_ROOT/a.log" "$TEST_ROOT/b.log" >&2
    echo "concurrent bootstrap failed: a=$status_a b=$status_b" >&2
    exit 1
fi

final="$TEST_ROOT/bin/baseup"
if ! cmp -s "$final" "$TEST_ROOT/source-a" && ! cmp -s "$final" "$TEST_ROOT/source-b"; then
    echo "final baseup is not a complete source fixture" >&2
    exit 1
fi

if compgen -G "$TEST_ROOT/bin/baseup.new.*" >/dev/null; then
    echo "temporary install files were left behind" >&2
    exit 1
fi

echo "baseup concurrent bootstrap install test passed"
