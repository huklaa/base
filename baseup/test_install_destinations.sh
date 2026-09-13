#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TEST_ROOT="$(mktemp -d)"
trap 'rm -rf "$TEST_ROOT"' EXIT

mkdir -p "$TEST_ROOT/mock-bin" "$TEST_ROOT/assets" "$TEST_ROOT/package"

cat > "$TEST_ROOT/mock-bin/curl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

url=""
output=""
while (( $# )); do
    case "$1" in
        -o)
            output="$2"
            shift 2
            ;;
        https://github.com/base/base/releases/download/*)
            url="$1"
            shift
            ;;
        *)
            shift
            ;;
    esac
done

[[ -n "$url" && -n "$output" ]]
printf '%s\n' "${url##*/}" >> "$TEST_ROOT/downloads"
cp "$TEST_ROOT/assets/${url##*/}" "$output"
EOF
chmod +x "$TEST_ROOT/mock-bin/curl"

export TEST_ROOT
export PATH="$TEST_ROOT/mock-bin:$PATH"
export BASEUP_HOME="$TEST_ROOT/state"

target="x86_64-unknown-linux-gnu"
for version in v1.3.0 v1.3.1; do
    printf '#!/usr/bin/env bash\nprintf '\''%s\\n'\'' '\''%s'\''\n' "$version" > "$TEST_ROOT/package/base"
    chmod +x "$TEST_ROOT/package/base"
    archive="base-${version}-${target}.tar.gz"
    tar -czf "$TEST_ROOT/assets/$archive" -C "$TEST_ROOT/package" base
    (cd "$TEST_ROOT/assets" && sha256sum "$archive" > "$archive.sha256")
done

run_install() {
    local destination="$1"
    local version="$2"

    BASE_BIN_DIR="$destination" \
        bash "$SCRIPT_DIR/baseup" --bin base --install "$version" --unsafe-skip-verify >/dev/null
}

bin_a="$TEST_ROOT/bin-a"
bin_b="$TEST_ROOT/bin-b"

# A legacy destination-agnostic entry must not suppress the first install after
# upgrading baseup, even if a binary already exists in the requested directory.
mkdir -p "$bin_a" "$BASEUP_HOME"
printf '#!/usr/bin/env bash\nprintf '\''v1.3.0\\n'\''\n' > "$bin_a/base"
chmod +x "$bin_a/base"
printf 'base:%s=v1.3.1\n' "$target" > "$BASEUP_HOME/installed-versions"
run_install "$bin_a" v1.3.1
[[ "$("$bin_a/base")" == "v1.3.1" ]]
[[ "$(wc -l < "$TEST_ROOT/downloads")" -eq 2 ]]
! grep -Fqx "base:$target=v1.3.1" "$BASEUP_HOME/installed-versions"

run_install "$bin_a" v1.3.0
run_install "$bin_b" v1.3.1
downloads_before="$(wc -l < "$TEST_ROOT/downloads")"

run_install "$bin_a" v1.3.1
[[ "$("$bin_a/base")" == "v1.3.1" ]]
[[ "$(wc -l < "$TEST_ROOT/downloads")" -eq $((downloads_before + 2)) ]]

downloads_before="$(wc -l < "$TEST_ROOT/downloads")"
run_install "$bin_a" v1.3.1
[[ "$(wc -l < "$TEST_ROOT/downloads")" -eq "$downloads_before" ]]

grep -Fqx "base:$target:$bin_a=v1.3.1" "$BASEUP_HOME/installed-versions"
grep -Fqx "base:$target:$bin_b=v1.3.1" "$BASEUP_HOME/installed-versions"
! grep -Fqx "base:$target=v1.3.1" "$BASEUP_HOME/installed-versions"

printf 'baseup install-destination tests passed\n'
