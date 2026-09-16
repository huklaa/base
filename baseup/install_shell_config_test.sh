#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
installer="$repo_root/baseup/install"
root="$(mktemp -d)"
trap 'rm -rf "$root"' EXIT

export HOME="$root/home"
export XDG_CONFIG_HOME="$HOME/.config"
export BASEUP_HOME="$root/base-home"
export BASE_BIN_DIR="$root/bin"
export SHELL=/bin/bash
export BASEUP_URL="https://example.invalid/baseup"
export FAKE_BASEUP="$root/fake-baseup"

mkdir -p "$XDG_CONFIG_HOME/fish" "$root/mock-bin"

cat > "$FAKE_BASEUP" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
chmod +x "$FAKE_BASEUP"

cat > "$root/mock-bin/curl" <<'EOF'
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
cp "$FAKE_BASEUP" "$out"
EOF
chmod +x "$root/mock-bin/curl"
export PATH="$root/mock-bin:$PATH"

source_line=". \"$BASEUP_HOME/env\""
fish_source_line="source \"$BASEUP_HOME/env.fish\""

cat > "$HOME/.bashrc" <<EOF
# user content
export KEEP_ME=1
# $source_line
EOF

cat > "$XDG_CONFIG_HOME/fish/config.fish" <<EOF
# user content
set -gx KEEP_ME 1
# $fish_source_line
EOF

bash "$installer" >/dev/null

grep -Fqx "$source_line" "$HOME/.bashrc"
grep -Fqx "$fish_source_line" "$XDG_CONFIG_HOME/fish/config.fish"
grep -Fqx 'export KEEP_ME=1' "$HOME/.bashrc"
grep -Fqx 'set -gx KEEP_ME 1' "$XDG_CONFIG_HOME/fish/config.fish"

# Running the installer again must not duplicate the active source lines.
bash "$installer" >/dev/null

[[ "$(grep -Fxc "$source_line" "$HOME/.bashrc")" -eq 1 ]]
[[ "$(grep -Fxc "$fish_source_line" "$XDG_CONFIG_HOME/fish/config.fish")" -eq 1 ]]

echo "install shell configuration regression test passed"
