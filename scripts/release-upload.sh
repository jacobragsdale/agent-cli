#!/usr/bin/env bash
# Upload a release that scripts/release-build.sh staged to the team's
# Artifact Keeper, then read latest/VERSION back the way install.sh will
# (anonymously).
#
#   KEEPER_URL=https://keeper.contoso.example KEEPER_TOKEN=... \
#       scripts/release-upload.sh target/dist
#
# KEEPER_URL is the server (the API is under /api/v1); KEEPER_TOKEN an API
# token with read:artifacts, write:artifacts and delete:artifacts on the
# repository, KEEPER_REPOSITORY (default agent-cli), a generic one that
# allows anonymous downloads.
#
# Keeper keeps an uploaded path immutable (a second PUT is 409), so a path
# that is there already is deleted and uploaded again: latest/ on every
# release, and a version's own files when its pipeline reruns. latest/VERSION
# goes last, so nothing reads a new version before its files are there.
set -euo pipefail

if [ $# -ne 1 ] || [ ! -f "$1/latest/VERSION" ]; then
    echo "usage: scripts/release-upload.sh DIST   (the directory release-build.sh printed)" >&2
    exit 2
fi
: "${KEEPER_URL:?release-upload: set KEEPER_URL, the Artifact Keeper server}"
: "${KEEPER_TOKEN:?release-upload: set KEEPER_TOKEN, an API token that may upload}"
dist=$1
repository=${KEEPER_REPOSITORY:-agent-cli}
api=${KEEPER_URL%/}/api/v1/repositories/$repository
# The token goes in a header file, not on curl's command line, which ps shows.
auth=$(mktemp)
trap 'rm -f "$auth"' EXIT
printf 'authorization: Bearer %s\n' "$KEEPER_TOKEN" >"$auth"
version=$(cat "$dist/latest/VERSION")

upload() {
    local path=$1 status
    status=$(curl -sS -o /dev/null -w '%{http_code}' -H "@$auth" -H 'content-type: application/octet-stream' \
        -T "$dist/$path" "$api/artifacts/$path")
    if [ "$status" = 409 ]; then
        status=$(curl -sS -o /dev/null -w '%{http_code}' -X DELETE -H "@$auth" "$api/artifacts/$path")
        case $status in
            2??) ;;
            *) echo "release-upload: $path is there already, and deleting it is HTTP $status; KEEPER_TOKEN needs delete:artifacts" >&2; exit 1 ;;
        esac
        status=$(curl -sS -o /dev/null -w '%{http_code}' -H "@$auth" -H 'content-type: application/octet-stream' \
            -T "$dist/$path" "$api/artifacts/$path")
    fi
    case $status in
        2??) echo "release-upload: $path" >&2 ;;
        401|403) echo "release-upload: $path: HTTP $status; KEEPER_TOKEN may not upload to $repository" >&2; exit 1 ;;
        404) echo "release-upload: $path: HTTP 404; is there a generic repository $repository at $KEEPER_URL?" >&2; exit 1 ;;
        *) echo "release-upload: $path: HTTP $status" >&2; exit 1 ;;
    esac
}

for file in "$dist/$version"/* "$dist"/latest/*; do
    path=${file#"$dist"/}
    [ "$path" = latest/VERSION ] || upload "$path"
done
upload latest/VERSION

served=$(curl -fsS "${KEEPER_URL%/}/api/v1/repositories/$repository/download/latest/VERSION") ||
    { echo "release-upload: latest/VERSION cannot be downloaded without a token; let the repository allow anonymous downloads" >&2; exit 1; }
[ "$served" = "$version" ] ||
    { echo "release-upload: latest/VERSION says $served, not $version" >&2; exit 1; }
echo "release-upload: agent-cli $version is latest at ${KEEPER_URL%/}/api/v1/repositories/$repository/download/latest/install.sh" >&2
