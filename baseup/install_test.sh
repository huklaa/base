#!/usr/bin/env bash
set -euo pipefail

installer="$(cd "$(dirname "$0")" && pwd)/install"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

mkdir -p "$tmp/mock-bin" "$tmp/home"

cat > "$tmp/mock-baseup" <<'MOCK_BASEUP'
#!/usr/bin/env bash
exit 0
MOCK_BASEUP
chmod +x "$tmp/mock-baseup"

cat > "$tmp/mock-bin/curl" <<'MOCK_CURL'
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
cp "$MOCK_BASEUP_FIXTURE" "$out"
MOCK_CURL
chmod +x "$tmp/mock-bin/curl"

run_bootstrap() {
    local bin_dir="$1"

    HOME="$tmp/home" \
        SHELL=/bin/bash \
        BASEUP_HOME="$tmp/state" \
        BASE_BIN_DIR="$bin_dir" \
        BASEUP_URL=https://example.invalid/baseup \
        MOCK_BASEUP_FIXTURE="$tmp/mock-baseup" \
        PATH="$tmp/mock-bin:/usr/bin:/bin" \
        bash "$installer" >/dev/null
}

bin_a="$tmp/bin-a"
bin_b="$tmp/bin-b"

run_bootstrap "$bin_a"

test -x "$bin_a/baseup"
grep -qF "$bin_a" "$tmp/state/env"
grep -qF "$bin_a" "$tmp/state/env.fish"

printf '%s\n' 'export BASEUP_TEST_KEEP=1' >> "$tmp/state/env"
printf '%s\n' 'set -gx BASEUP_TEST_KEEP 1' >> "$tmp/state/env.fish"

run_bootstrap "$bin_b"

test -x "$bin_b/baseup"
grep -qF "$bin_b" "$tmp/state/env"
grep -qF "$bin_b" "$tmp/state/env.fish"
! grep -qF "$bin_a" "$tmp/state/env"
! grep -qF "$bin_a" "$tmp/state/env.fish"
grep -qFx 'export BASEUP_TEST_KEEP=1' "$tmp/state/env"
grep -qFx 'set -gx BASEUP_TEST_KEEP 1' "$tmp/state/env.fish"

echo "baseup install PATH refresh test passed"
