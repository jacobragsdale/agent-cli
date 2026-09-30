#!/usr/bin/env bash
# Start SQL Server and Oracle, wait until both accept logins, load
# scripts/seed into each and print one summary line per database.
# Idempotent: both seed files skip what already exists.
#
# sql-bench's compose runs the same two images on the same ports; when its
# containers are already up they are used as they are rather than fought
# over.
set -euo pipefail
cd "$(dirname "$0")/.."

SA_PASSWORD='Bench_Pass1!'   # throwaway, localhost only

if docker ps --format '{{.Names}}' | grep -qx sql-bench-mssql; then
    prefix=sql-bench
    echo "db-up: sql-bench's containers are running; using them"
else
    prefix=agent-cli
    docker compose up -d
fi

mssql() {
    docker exec "$prefix-mssql" /opt/mssql-tools18/bin/sqlcmd \
        -C -b -S localhost -U sa -P "$SA_PASSWORD" "$@"
}

oracle() {
    docker exec -i -e NLS_LANG=.AL32UTF8 "$prefix-oracle" sqlplus -s bench/bench@FREEPDB1 "$@"
}

wait_healthy() {
    local name=$1 deadline=$((SECONDS + 300)) status=starting
    while [ "$status" != healthy ]; do
        if [ "$SECONDS" -ge "$deadline" ]; then
            echo "db-up: $name is $status after 5 minutes" >&2
            docker logs --tail 30 "$name" >&2 || true
            exit 1
        fi
        sleep 2
        status=$(docker inspect -f '{{.State.Health.Status}}' "$name" 2>/dev/null || echo missing)
    done
}

# sqlplus reports most failures in its output rather than its exit status.
check() {
    if grep -qE 'ORA-|PLS-|SP2-|Msg [0-9]+|compilation errors' <<<"$1"; then
        echo "$1" >&2
        exit 1
    fi
}

wait_healthy "$prefix-mssql"
wait_healthy "$prefix-oracle"

docker cp scripts/seed/mssql.sql "$prefix-mssql:/tmp/agent-cli-seed.sql"
out=$(mssql -i /tmp/agent-cli-seed.sql 2>&1) || { echo "$out" >&2; exit 1; }
check "$out"
out=$(oracle <scripts/seed/oracle.sql 2>&1) || { echo "$out" >&2; exit 1; }
check "$out"

mssql_count=$(mssql -d bench -h -1 -W -Q "set nocount on; select count(*) from bench.customers" | tr -dc '0-9\n' | grep -m1 .)
oracle_count=$(printf 'set heading off feedback off pagesize 0\nselect count(*) from customers;\nexit\n' | oracle | tr -dc '0-9\n' | grep -m1 .)
for pair in "mssql:$mssql_count" "oracle:$oracle_count"; do
    [ "${pair#*:}" = 50 ] || { echo "db-up: ${pair%%:*} seed is wrong: ${pair#*:} customers" >&2; exit 1; }
done
echo "mssql   localhost:1433 db=bench     50 customers ($prefix-mssql)"
echo "oracle  localhost:1521 FREEPDB1     50 customers ($prefix-oracle)"
echo "run:    AGENT_CLI_TEST_DBS=1 cargo test --workspace"
