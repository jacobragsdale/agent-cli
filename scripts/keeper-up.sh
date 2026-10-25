#!/usr/bin/env bash
# Start the throwaway Artifact Keeper (scripts/keeper/compose.yaml) on
# 127.0.0.1:18070, wait until it answers, and seed it. Every secret here is
# throwaway; admin's password is Keeper_Pass1!.
#
#   eval "$(scripts/keeper-up.sh)"   start (again), seed, export the env below
#   scripts/keeper-up.sh down        stop it and drop its volumes
#
# What it seeds (idempotent; a rerun keeps what is there):
#   agent-cli  a generic repository, local, public: anonymous downloads, the
#              way scripts/install.sh reads releases at work
#   pypi       a PyPI repository, local, public: contoso-orders-client 2.2.0
#              and 2.3.0, uploaded as twine does (the legacy upload API)
#   builds     a generic repository, local, public: api/1.4.1/ and api/1.4.2/,
#              a tarball each and 1.4.2's .sha256
#   internal   a generic repository, local, private: etl/2.0.1/etl-2.0.1.whl
#   pypi-proxy a PyPI repository, remote, public, proxying https://pypi.org,
#              with six 1.17.0's wheel fetched through it when pypi.org answers
#   pypi-all   a PyPI repository, virtual, public: pypi, then pypi-proxy
#   a token    named agent-cli-dev, scope *, 30 days, made anew on every run
#              (the old one is revoked), since Keeper shows a token only once
#
# The keeper domain's live suite (crates/cli/tests/live_keeper.rs) reads all
# of them.
#
# It prints, on stdout and nothing else:
#   KEEPER_URL         http://127.0.0.1:18070 (the API is under /api/v1)
#   KEEPER_TOKEN       the token above, sent as Authorization: Bearer
#   AGENT_CLI_RELEASES the agent-cli repository's download URL, for install.sh
#
# KEEPER_VERSION picks the backend image's tag (default 1.11.7) and
# KEEPER_PORT the port.
set -euo pipefail
cd "$(dirname "$0")/keeper"

compose=(docker compose -f compose.yaml)
if [ "${1:-}" = down ]; then
    exec "${compose[@]}" down -v
fi

port=${KEEPER_PORT:-18070}
url=http://127.0.0.1:$port
api=$url/api/v1
"${compose[@]}" up -d --wait >&2

# Reads JSON on stdin as d and runs the Python statement it is given.
json() { python3 -I -c "import json, sys; d = json.load(sys.stdin); $1"; }

jwt=$(curl -fsS "$api/auth/login" -H 'content-type: application/json' \
    -d '{"username":"admin","password":"Keeper_Pass1!"}' | json 'print(d["access_token"])')
auth=(-H "authorization: Bearer $jwt")

for id in $(curl -fsS "${auth[@]}" "$api/users/me/tokens" |
    json 'print(*[t["id"] for t in d["items"] if t["name"] == "agent-cli-dev"])'); do
    curl -fsS -o /dev/null -X DELETE "${auth[@]}" "$api/users/me/tokens/$id"
done
token=$(curl -fsS "${auth[@]}" "$api/users/me/tokens" -H 'content-type: application/json' \
    -d '{"name":"agent-cli-dev","scopes":["*"],"expires_in_days":30}' | json 'print(d["token"])')

# repository KEY FORMAT [TYPE [VISIBILITY [UPSTREAM]]]: made unless it is there.
repository() {
    local key=$1 format=$2 type=${3:-local} visibility=${4:-public} upstream=${5:-} extra=
    if ! curl -fsS -o /dev/null "${auth[@]}" "$api/repositories/$key" 2>/dev/null; then
        [ -n "$upstream" ] && extra=",\"upstream_url\":\"$upstream\""
        curl -fsS -o /dev/null "${auth[@]}" "$api/repositories" -H 'content-type: application/json' \
            -d "{\"key\":\"$key\",\"name\":\"$key\",\"format\":\"$format\",\"repo_type\":\"$type\",\"visibility\":\"$visibility\"$extra}"
    fi
}
repository agent-cli generic
repository pypi pypi
repository builds generic
repository internal generic local private
repository pypi-proxy pypi remote public https://pypi.org
repository pypi-all pypi virtual
curl -fsS -o /dev/null -X PUT "${auth[@]}" "$api/repositories/pypi-all/members" -H 'content-type: application/json' \
    -d '{"members":[{"member_key":"pypi","priority":1},{"member_key":"pypi-proxy","priority":2}]}'

seed=$(mktemp -d)
trap 'rm -rf "$seed"' EXIT

# upload REPO PATH CONTENT-TYPE TEXT: Keeper refuses a path twice (409),
# which a rerun meets and keeps.
upload() {
    printf '%s\n' "$4" >"$seed/body"
    local status
    status=$(curl -sS -o /dev/null -w '%{http_code}' -X PUT "${auth[@]}" -H "content-type: $3" \
        --data-binary "@$seed/body" "$api/repositories/$1/artifacts/$2")
    case $status in
        200 | 201 | 409) ;;
        *) echo "keeper-up: PUT $1/$2 answered $status" >&2 && exit 1 ;;
    esac
}
upload builds api/1.4.1/api-1.4.1-linux-x64.tar.gz application/gzip "api 1.4.1"
upload builds api/1.4.2/api-1.4.2-linux-x64.tar.gz application/gzip "api 1.4.2"
upload builds api/1.4.2/api-1.4.2-linux-x64.tar.gz.sha256 text/plain "api 1.4.2 sha256"
upload internal etl/2.0.1/etl-2.0.1.whl application/zip "etl 2.0.1"

# pypi_upload NAME VERSION: an sdist holding only PKG-INFO, sent as twine
# sends it, unless the version is there.
pypi_upload() {
    local name=$1 version=$2 dir file
    curl -fsS -o /dev/null "$url/pypi/pypi/pypi/$name/$version/json" 2>/dev/null && return
    dir="${name//-/_}-$version"
    file="$seed/$dir.tar.gz"
    mkdir -p "$seed/$dir"
    printf 'Metadata-Version: 2.1\nName: %s\nVersion: %s\nSummary: Orders API client\n' \
        "$name" "$version" >"$seed/$dir/PKG-INFO"
    tar -C "$seed" -czf "$file" "$dir"
    curl -fsS -o /dev/null -H "authorization: Bearer $token" "$url/pypi/pypi/" \
        -F ':action=file_upload' -F 'protocol_version=1' -F 'metadata_version=2.1' \
        -F "name=$name" -F "version=$version" -F 'filetype=sdist' -F 'pyversion=source' \
        -F 'summary=Orders API client' -F "sha256_digest=$(sha256sum "$file" | cut -d' ' -f1)" \
        -F "content=@$file"
}
pypi_upload contoso-orders-client 2.2.0
pypi_upload contoso-orders-client 2.3.0

# What pip would fetch through the proxy; offline, the proxy holds nothing.
curl -fsS -o /dev/null "$url/pypi/pypi-proxy/simple/six/six-1.17.0-py2.py3-none-any.whl" ||
    echo "keeper-up: pypi.org did not answer; pypi-proxy has cached nothing" >&2

echo "keeper-up: Artifact Keeper $(curl -fsS "$url/health" | json 'print(d["version"])') on $url: repositories agent-cli, builds, internal (generic), pypi, pypi-proxy, pypi-all; token agent-cli-dev" >&2
echo "export KEEPER_URL=$url"
echo "export KEEPER_TOKEN=$token"
echo "export AGENT_CLI_RELEASES=$api/repositories/agent-cli/download"
