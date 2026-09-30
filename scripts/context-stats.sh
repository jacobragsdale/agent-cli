#!/bin/sh
# What a change costs an agent in reading: the largest source files, lines per
# crate, and for each of the last N commits (default 20) how many files it
# touched outside the domain crate its subject names.
#
#   scripts/context-stats.sh [N]
#
# A subject names a crate by its first domain word (kv, acr and aks are the
# azure crate); a subject naming none counts every file as outside.
set -eu
cd "$(dirname "$0")/.."
n=${1:-20}

echo "## Largest source files (lines)"
wc -l crates/*/src/*.rs crates/*/src/*/*.rs crates/*/src/*/*/*.rs crates/*/src/*/*/*/*.rs 2>/dev/null |
    grep -v ' total$' | sort -rn | head -15

echo
echo "## Lines per crate (src and tests)"
for crate in crates/*/; do
    lines=$(find "$crate" -name '*.rs' -exec cat {} + | wc -l)
    printf '%7d %s\n' "$lines" "${crate%/}"
done | sort -rn

echo
echo "## Files touched outside the named crate, last $n commits"
echo "outside/total crate   commit subject"
git log --no-merges --format='%h %s' -"$n" | while read -r hash subject; do
    crate=-
    for word in $(echo "$subject" | tr 'A-Z' 'a-z' | tr -c 'a-z0-9\n' ' '); do
        case $word in
            ado | sql | k8s | airflow | dd | core | cli) crate=$word ;;
            kv | acr | aks | azure) crate=azure ;;
            *) continue ;;
        esac
        break
    done
    total=$(git show --name-only --format= "$hash" | grep -c . || true)
    if [ "$crate" = - ]; then
        outside=$total
    else
        outside=$(git show --name-only --format= "$hash" | grep -vc "^crates/$crate/" || true)
    fi
    printf '%7s %-7s %s %.70s\n' "$outside/$total" "$crate" "$hash" "$subject"
done
