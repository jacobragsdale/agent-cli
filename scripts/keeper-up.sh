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
#   pypi       a PyPI repository, local, public, empty
#   a token    named agent-cli-dev, scope *, 30 days, made anew on every run
#              (the old one is revoked), since Keeper shows a token only once
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

repository() {
    local key=$1 format=$2
    if ! curl -fsS -o /dev/null "${auth[@]}" "$api/repositories/$key"; then
        curl -fsS -o /dev/null "${auth[@]}" "$api/repositories" -H 'content-type: application/json' \
            -d "{\"key\":\"$key\",\"name\":\"$key\",\"format\":\"$format\",\"repo_type\":\"local\",\"visibility\":\"public\"}"
    fi
}
repository agent-cli generic
repository pypi pypi

echo "keeper-up: Artifact Keeper $(curl -fsS "$url/health" | json 'print(d["version"])') on $url: repositories agent-cli (generic), pypi; token agent-cli-dev" >&2
echo "export KEEPER_URL=$url"
echo "export KEEPER_TOKEN=$token"
echo "export AGENT_CLI_RELEASES=$api/repositories/agent-cli/download"
