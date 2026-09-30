#!/bin/sh
# Writes a new command from scripts/templates/command.rs, in the layout every
# crate follows: crates/<crate>/src/<resource>/<verb>.rs, with the domain as a
# first directory in a crate holding several domains (azure).
#
#   scripts/new-command.sh DOMAIN RESOURCE VERB --effect read|write|destructive|reveal|varies
#
# It also declares the module (creating <resource>/mod.rs when needed), adds
# the command to the domain's `commands` in lib.rs (last, so move it to where
# it belongs in the listing), bumps the domain's command-count test, and
# appends two placeholder queries to the crate's search.toml. The file
# compiles; its tests and the search gate fail until every TODO is filled in.
set -eu
cd "$(dirname "$0")/.."

fail() {
    echo "new-command: $1" >&2
    exit 2
}

[ $# -eq 5 ] && [ "$4" = --effect ] ||
    fail "usage: scripts/new-command.sh DOMAIN RESOURCE VERB --effect read|write|destructive|reveal|varies"
domain=$1 resource=$2 verb=$3 effect=$5

case $effect in
    read) Effect=Read kind=read yes= ;;
    reveal) Effect=Reveal kind=read yes=--reveal ;;
    write) Effect=Write kind=write yes= write=Write ;;
    destructive) Effect=Destructive kind=write yes=--yes write=Destructive ;;
    varies) Effect=Varies kind=write yes=--yes write=Destructive ;;
    *) fail "--effect is one of read, write, destructive, reveal, varies" ;;
esac

lib=$(grep -l "^    name: \"$domain\",$" crates/*/src/lib.rs | head -n 1 || true)
domains=$(grep -h '^    name: "' crates/*/src/lib.rs | cut -d'"' -f2 | sort | tr '\n' ' ')
[ -n "$lib" ] || fail "no domain $domain; the domains are: $domains"
verbs=$(sed -n '/^pub const VERBS/,/^];/p' crates/core/src/registry.rs | grep -o '"[a-z-]*"' | tr -d '"' | tr '\n' ' ')
case " $verbs " in
    *" $verb "*) ;;
    *) fail "no verb $verb; the verbs are: $verbs(a new one is a deliberate edit to VERBS in crates/core/src/registry.rs)" ;;
esac
echo "$resource" | grep -Eqx '[a-z][a-z0-9]*(-[a-z0-9]+)*' || fail "the resource is a lowercase kebab word: $resource"

crate=$(dirname "$(dirname "$lib")")
# The Domain constant whose name is DOMAIN, and how many the crate holds.
const=$(grep -B1 "^    name: \"$domain\",$" "$lib" | sed -n 's/^pub const \([A-Z0-9_]*\): Domain.*/\1/p')
[ "$(grep -c '^pub const [A-Z0-9_]*: Domain' "$lib")" -gt 1 ] && prefix="$domain/" || prefix=
res=$(echo "$resource" | tr - _)
dir="$crate/src/$prefix$res"
file="$dir/$verb.rs"
[ -e "$file" ] && fail "$file exists; refusing to overwrite it"

upper() { echo "$1" | tr 'a-z-' 'A-Z_'; }
camel() { echo "$1" | sed -E 's/(^|[-_])([a-z])/\U\2/g'; }
name=$(upper "$res")_$(upper "$verb")
handler=${res}_$verb
struct=$(camel "$res")$(camel "$verb")
[ "$verb" = list ] && shape=list || shape=one
[ -n "$yes" ] && yesmark=yes || yesmark=noyes

# Declares `child` in the module file of `dir` (`dir.rs`, else `dir/mod.rs`,
# made when neither exists and declared in its own parent in turn).
declare_mod() { # parent_dir child
    parent=$1 child=$2
    if [ "$parent" = "$crate/src" ]; then
        target=$lib line="mod $child;"
    elif [ -e "$parent.rs" ]; then
        target=$parent.rs line="pub(crate) mod $child;"
    else
        target=$parent/mod.rs line="pub(crate) mod $child;"
        if [ ! -e "$target" ]; then
            mkdir -p "$parent"
            printf '//! TODO: what `%s` holds that its commands share.\n' "$(basename "$parent" | tr _ -)" >"$target"
            declare_mod "$(dirname "$parent")" "$(basename "$parent")"
        fi
    fi
    grep -Eq "^(pub\(crate\) )?mod $child;" "$target" && return
    if grep -Eq '^(pub\(crate\) )?mod [a-z0-9_]+;' "$target"; then
        awk -v line="$line" 'NR == FNR { if ($0 ~ /^(pub\(crate\) )?mod [a-z0-9_]+;/) last = FNR; next }
            { print } FNR == last { print line }' "$target" "$target" >"$target.new"
    else
        awk -v line="$line" '!done && !/^\/\/!/ { print ""; print line; done = 1 } { print }
            END { if (!done) { print ""; print line } }' "$target" >"$target.new"
    fi
    mv "$target.new" "$target"
}

mkdir -p "$dir"
sed -e "s|__PATH__|$domain $resource $verb|g" -e "s|__DOMAIN_CONST__|$const|g" \
    -e "s|__DOMAIN__|$domain|g" -e "s|__RESOURCE__|$resource|g" -e "s|__VERB__|$verb|g" \
    -e "s|__CONST__|$name|g" -e "s|__EFFECT__|$Effect|g" -e "s|__HANDLER__|$handler|g" \
    -e "s|__ARGS__|${struct}Args|g" -e "s|__ROW__|${struct}Row|g" \
    -e "s|__YES_ARG__|$yes|g" -e "s|__YES__|${yes:+ $yes}|g" \
    -e "s|Effect::Destructive, request|Effect::${write:-Destructive}, request|" \
    scripts/templates/command.rs |
    awk -v keep=" $kind $shape $yesmark " '
        match($0, / \/\/@[a-z]+$/) {
            mark = substr($0, RSTART + 4)
            if (index(keep, " " mark " ")) print substr($0, 1, RSTART - 1)
            next
        }
        { print }' >"$file"
declare_mod "$(dirname "$dir")" "$res"
declare_mod "$dir" "$verb"

# The command goes last in its domain's `commands`, and the count test moves.
modpath=$(echo "$prefix$res" | tr / :  | sed 's/:/::/g')
awk -v const="$const" -v entry="        $modpath::$verb::$name," '
    $0 ~ "^pub const " const ": Domain" { inside = 1 }
    inside && /^    commands: &\[/ { commands = 1 }
    commands && /^    \],$/ { print entry; commands = 0; inside = 0 }
    { print }' "$lib" >"$lib.new" && mv "$lib.new" "$lib"
awk -v const="$const" '
    index($0, "assert_eq!(" const ".commands.len(), ") {
        n = $0; sub(/.*commands\.len\(\), /, "", n); sub(/\).*/, "", n)
        sub("commands.len\\(\\), " n "\\)", "commands.len(), " n + 1 ")")
    }
    { print }' "$lib" >"$lib.new" && mv "$lib.new" "$lib"

cat >>"$crate/search.toml" <<TOML

[[query]]
text = "TODO a task an agent would be given that $domain $resource $verb answers"
expect = "$domain $resource $verb"

[[query]]
text = "TODO another wording of it, as an agent would search"
expect = "$domain $resource $verb"
TOML

rustfmt --edition 2024 "$file" "$lib"
echo "wrote $file; fill in its TODOs, the two queries in $crate/search.toml, and move"
echo "$modpath::$verb::$name to its place in $lib (the listing order)"
