#!/usr/bin/env bash
set -u -o pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
INSTALL_SCRIPT="${INSTALL_SCRIPT:-$SCRIPT_DIR/install}"

test_root="$(mktemp -d)"
trap 'rm -rf "$test_root"' EXIT
barrier_dir="$test_root/barrier"
mkdir -p "$barrier_dir" "$test_root/bin"

# Load the real installer functions without executing its final main call.
# shellcheck disable=SC1090
source <(sed '$d' "$INSTALL_SCRIPT")
set +e

printf '#!/bin/sh\necho one\n' > "$test_root/one"
printf '#!/bin/sh\necho two\n' > "$test_root/two"
chmod +x "$test_root/one" "$test_root/two"
destination="$test_root/bin/baseup"

wait_for_copy_barrier() {
    local attempts=0
    while [[ "$(find "$barrier_dir" -name 'cp-*' -type f | wc -l)" -lt 2 ]]; do
        attempts=$((attempts + 1))
        if [[ "$attempts" -ge 200 ]]; then
            echo "timed out waiting for concurrent copies" >&2
            return 1
        fi
        sleep 0.01
    done
}

wait_for_second_move() {
    local attempts=0
    while [[ ! -f "$barrier_dir/second-moved" ]]; do
        attempts=$((attempts + 1))
        if [[ "$attempts" -ge 200 ]]; then
            echo "timed out waiting for the second install to move its staging file" >&2
            return 1
        fi
        sleep 0.01
    done
}

cp() {
    command cp "$@" || return
    touch "$barrier_dir/cp-$INSTALL_ID"
    wait_for_copy_barrier
}

chmod() {
    if [[ "$INSTALL_ID" == 1 ]]; then
        wait_for_second_move || return
    fi
    command chmod "$@"
}

mv() {
    command mv "$@" || return
    if [[ "$INSTALL_ID" == 2 ]]; then
        touch "$barrier_dir/second-moved"
    fi
}

(
    INSTALL_ID=1
    install_bin "$test_root/one" "$destination"
) &
first_pid=$!
(
    INSTALL_ID=2
    install_bin "$test_root/two" "$destination"
) &
second_pid=$!

wait "$first_pid"; first_status=$?
wait "$second_pid"; second_status=$?

if [[ "$first_status" -ne 0 || "$second_status" -ne 0 ]]; then
    echo "concurrent installs interfered: first=$first_status second=$second_status" >&2
    exit 1
fi

if [[ ! -x "$destination" ]]; then
    echo "destination was not installed" >&2
    exit 1
fi

echo "concurrent install staging test passed"
