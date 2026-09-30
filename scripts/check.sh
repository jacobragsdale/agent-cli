#!/bin/sh
# The checks, quietly: prints only a failing step's output, then one line.
#
#   scripts/check.sh ado sql   # fmt, then per crate clippy and tests, then the gates
#   scripts/check.sh --all     # exactly CI's five checks
#
# The gates are crates/cli's tests with the fixtures feature: the registry
# rules, the search gate, read-only refusal, the reference and the world.
# Stops at the first failure, exiting non-zero.
set -eu
cd "$(dirname "$0")/.."

usage() {
    echo "usage: scripts/check.sh CRATE... | --all   (crates: $(ls crates | tr '\n' ' '))" >&2
    exit 2
}
[ $# -gt 0 ] || usage

out=$(mktemp)
trap 'rm -f "$out"' EXIT
steps=0
step() {
    steps=$((steps + 1))
    if ! "$@" >"$out" 2>&1; then
        echo "check: failed: $*" >&2
        cat "$out" >&2
        exit 1
    fi
}

if [ "$1" = --all ]; then
    [ $# -eq 1 ] || usage
    step cargo fmt --all -- --check
    step cargo clippy -q --workspace --all-targets -- -D warnings
    step cargo clippy -q --workspace --all-targets --features agent-cli/fixtures -- -D warnings
    step cargo test -q --workspace
    step cargo test -q --workspace --features agent-cli/fixtures
    echo "check: all ok ($steps steps)"
    exit 0
fi

for crate in "$@"; do
    [ -d "crates/$crate" ] || usage
done
step cargo fmt --all -- --check
for crate in "$@"; do
    package=agent-cli-$crate
    [ "$crate" = cli ] && package=agent-cli
    step cargo clippy -q -p "$package" --all-targets -- -D warnings
    step cargo test -q -p "$package"
done
step cargo clippy -q -p agent-cli --all-targets --features fixtures -- -D warnings
step cargo test -q -p agent-cli --features fixtures
echo "check: $* ok ($steps steps)"
