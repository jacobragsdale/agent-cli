#!/usr/bin/env bash
# Build the release and stage what goes to the team's Artifact Keeper, in the
# layout scripts/install.sh reads:
#
#   DIST/VERSION/agent-cli-x86_64-linux.tar.gz          the binary, alone
#   DIST/VERSION/agent-cli-x86_64-linux.tar.gz.sha256   sha256sum's line for it
#   DIST/VERSION/team.toml                              team/team.toml
#   DIST/VERSION/install.sh                             scripts/install.sh
#   DIST/latest/…                                       the same four, and
#   DIST/latest/VERSION                                 the version, which
#                                                       doctor and install.sh read
#
#   scripts/release-build.sh          stage the workspace version
#   scripts/release-build.sh v0.2.0   the same, refusing a tag that is not
#                                     v + the workspace version
#
# DIST is $CARGO_TARGET_DIR/dist (target/dist); the script prints it. Build in
# ci/Dockerfile's image: the binary must need glibc 2.35 at most (Ubuntu
# 22.04), which this checks. Then scripts/release-upload.sh DIST.
set -euo pipefail
cd "$(dirname "$0")/.."

# The [workspace.package] version, the one every crate inherits.
version=$(awk '/^\[/ { section = $0 } section == "[workspace.package]" && /^version *=/ {
    gsub(/[" ]/, "", $0); sub(/^version=/, "", $0); print; exit }' Cargo.toml)
[ -n "$version" ] || { echo "release-build: no [workspace.package] version in Cargo.toml" >&2; exit 1; }
if [ $# -gt 0 ] && [ "$1" != "v$version" ]; then
    echo "release-build: tag $1 is not v$version, Cargo.toml's [workspace.package] version; bump it or tag v$version" >&2
    exit 2
fi

releases=$(sed -n 's/^releases *= *"\([^"]*\)".*/\1/p' team/team.toml | head -n 1)
[ -n "$releases" ] || { echo "release-build: team/team.toml has no [core] releases" >&2; exit 1; }

cargo build --release --locked -p agent-cli
target=${CARGO_TARGET_DIR:-target}
binary=$target/release/agent-cli

# The newest GLIBC_x.y symbol version the binary asks for.
glibc=$(objdump -T "$binary" | grep -o 'GLIBC_[0-9.]*' | sed 's/GLIBC_//' | sort -V | tail -n 1)
if [ "$(printf '%s\n' "$glibc" 2.35 | sort -V | tail -n 1)" != 2.35 ]; then
    echo "release-build: the binary needs glibc $glibc, newer than Ubuntu 22.04's 2.35; build it in ci/Dockerfile's image" >&2
    exit 1
fi

dist=$target/dist
rm -rf "$dist"
mkdir -p "$dist/$version"
tarball=agent-cli-x86_64-linux.tar.gz
tar -C "$target/release" --owner=0 --group=0 --numeric-owner -czf "$dist/$version/$tarball" agent-cli
(cd "$dist/$version" && sha256sum "$tarball" >"$tarball.sha256")
cp team/team.toml "$dist/$version/team.toml"
# install.sh's default releases URL is team.toml's, so the fork fills it in once.
sed "s|^RELEASES_DEFAULT=.*|RELEASES_DEFAULT=$releases|" scripts/install.sh >"$dist/$version/install.sh"
grep -qx "RELEASES_DEFAULT=$releases" "$dist/$version/install.sh" ||
    { echo "release-build: scripts/install.sh has no RELEASES_DEFAULT= line" >&2; exit 1; }
cp -R "$dist/$version" "$dist/latest"
echo "$version" >"$dist/latest/VERSION"

echo "release-build: agent-cli $version (glibc $glibc at most), releases $releases" >&2
echo "$dist"
