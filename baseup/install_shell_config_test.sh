#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
installer="$repo_root/baseup/install"
test_root="$(mktemp -d)"
trap 'rm -rf "$test_root"' EXIT

export HOME="$test_root/home"
export SHELL=/bin/bash
export BASEUP_HOME="$HOME/.base"
export BASE_BIN_DIR="$BASEUP_HOME/bin"
export XDG_CONFIG_HOME="$HOME/.config"

mkdir -p "$HOME" "$XDG_CONFIG_HOME/fish" "$test_root/mock-bin"

cat > "$test_root/mock-bin/curl" <<'MOCK'
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
cat > "$out" <<'SCRIPT'
#!/usr/bin/env bash
exit 0
SCRIPT
MOCK
chmod +x "$test_root/mock-bin/curl"
export PATH="$test_root/mock-bin:$PATH"

printf '%s\n' \
    '# existing bash config' \
    "# Base env path for reference: $BASEUP_HOME/env" \
    > "$HOME/.bashrc"

printf '%s\n' \
    '# existing fish config' \
    "# Base env path for reference: $BASEUP_HOME/env.fish" \
    > "$XDG_CONFIG_HOME/fish/config.fish"

run_installer() {
    BASEUP_URL=https://example.invalid/baseup bash "$installer" >/dev/null
}

run_installer

posix_source=". \"$BASEUP_HOME/env\""
fish_source="source \"$BASEUP_HOME/env.fish\""

grep -Fxq "$posix_source" "$HOME/.bashrc"
grep -Fxq "$fish_source" "$XDG_CONFIG_HOME/fish/config.fish"
[[ "$(grep -Fxc "$posix_source" "$HOME/.bashrc")" -eq 1 ]]
[[ "$(grep -Fxc "$fish_source" "$XDG_CONFIG_HOME/fish/config.fish")" -eq 1 ]]

run_installer

[[ "$(grep -Fxc "$posix_source" "$HOME/.bashrc")" -eq 1 ]]
[[ "$(grep -Fxc "$fish_source" "$XDG_CONFIG_HOME/fish/config.fish")" -eq 1 ]]

echo "baseup shell configuration tests passed"
