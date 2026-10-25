#!/usr/bin/env bash
# Start the throwaway Confluence Data Center that
# crates/cli/tests/live_confluence.rs runs against, behind the /confluence
# context path on 127.0.0.1:18090, with Postgres beside it, and seed it
# (scripts/seed/confluence.py). Every password here is throwaway.
#
#   scripts/confluence-up.sh          start it afresh and seed it
#   scripts/confluence-up.sh down     remove both containers
#
# The licence is Atlassian's public 3-hour timebomb licence for testing
# (developer.atlassian.com, "Timebomb licenses for testing server apps"),
# read from that page unless CONFLUENCE_LICENSE holds one; past three hours
# the site stops taking writes, so run this again. CONFLUENCE_VERSION picks
# the image (default 9.2, the long-term release). It wants about 3.5 GB of
# memory: build first (cargo test --no-run), since earlyoom kills rustc.
#
# Then: AGENT_CLI_TEST_CONFLUENCE=1 cargo test -p agent-cli --test live_confluence
set -euo pipefail
cd "$(dirname "$0")/.."

NET=agent-cli-confluence
docker rm -f agent-cli-confluence agent-cli-confluence-db >/dev/null 2>&1 || true
if [ "${1:-}" = down ]; then
    docker network rm "$NET" >/dev/null 2>&1 || true
    exit 0
fi

license=${CONFLUENCE_LICENSE:-}
if [ -z "$license" ]; then
    license=$(curl -fsSL https://developer.atlassian.com/platform/marketplace/timebomb-licenses-for-testing-server-apps/ |
        python3 -I -c '
import re, sys
page = sys.stdin.read()
at = page.find("Confluence Data Center license, expires in 3 hours")
found = re.search(r"```[^\\]*\\n(.*?)\\n```", page[at:at + 4000]) if at >= 0 else None
print(found.group(1).replace("\\n", "\n").strip() if found else "")')
fi
if [ -z "$license" ]; then
    echo "confluence-up: no licence found on Atlassian's timebomb page; set CONFLUENCE_LICENSE" >&2
    exit 1
fi

docker network create "$NET" >/dev/null 2>&1 || true
docker run -d --name agent-cli-confluence-db --network "$NET" \
    -e POSTGRES_USER=confluence -e POSTGRES_PASSWORD=confluence -e POSTGRES_DB=confluence \
    -e POSTGRES_INITDB_ARGS="--encoding=UTF8 --lc-collate=C --lc-ctype=C" postgres:16-alpine >/dev/null
until docker exec agent-cli-confluence-db pg_isready -U confluence >/dev/null 2>&1; do sleep 1; done
docker run -d --name agent-cli-confluence --network "$NET" -p 127.0.0.1:18090:8090 \
    -e ATL_JDBC_URL=jdbc:postgresql://agent-cli-confluence-db:5432/confluence \
    -e ATL_JDBC_USER=confluence -e ATL_JDBC_PASSWORD=confluence -e ATL_DB_TYPE=postgresql \
    -e ATL_LICENSE_KEY="$license" -e ATL_TOMCAT_CONTEXTPATH=/confluence \
    -e JVM_MINIMUM_MEMORY=1024m -e JVM_MAXIMUM_MEMORY=2048m \
    "atlassian/confluence:${CONFLUENCE_VERSION:-9.2}" >/dev/null

python3 -I scripts/seed/confluence.py http://127.0.0.1:18090/confluence
echo "confluence-up: AGENT_CLI_TEST_CONFLUENCE=1 cargo test -p agent-cli --test live_confluence"
