#!/usr/bin/env bash
# Seeds the throwaway Airflow scripts/airflow-up.sh starts, from inside its
# scheduler container: variables, a pool with no slots, connections (an
# mssql and an oracle one on hosts live_airflow.rs's [[sql.connection]]s
# name), a DAG whose file then disappears (stale), and one run of each e2e
# DAG. Idempotent: a DAG that has runs is not triggered again.
set -euo pipefail

# The CLI warns about every provider on every call; only failures matter.
exec 3>&2 2>/tmp/seed.err
trap 'status=$?; [ "$status" -eq 0 ] || cat /tmp/seed.err >&3' EXIT

airflow variables set --description "Where extract_orders writes" e2e_orders_bucket s3://contoso-orders >/dev/null
airflow variables set --description "A token a DAG reads" e2e_api_token not-a-real-token >/dev/null
airflow pools set e2e_zero 0 "No slots, so its tasks wait" >/dev/null

# --conn-json keeps mssql and oracle, which --conn-type turns into generic
# when their providers are not installed.
conn() {
    airflow connections delete "$1" >/dev/null 2>&1 || true
    airflow connections add "$1" --conn-json "$2" >/dev/null 2>&1
}
conn e2e_mssql '{"conn_type": "mssql", "host": "mssql.contoso.example", "port": 1433, "schema": "orders",
    "login": "etl", "password": "not-a-real-password", "description": "Orders warehouse"}'
conn e2e_oracle '{"conn_type": "oracle", "host": "oracle.contoso.example", "port": 1521,
    "schema": "FREEPDB1", "login": "etl", "password": "not-a-real-password"}'
conn e2e_http '{"conn_type": "http", "host": "api.contoso.example"}'

# Until DAG $1 is parsed (Airflow 3's dag processor parses on its own time).
parsed() {
    for _ in $(seq 40); do
        airflow dags list -o plain 2>/dev/null | grep -q "^$1\b" && return
        sleep 3
    done
    echo "seed: DAG $1 was never parsed" >&3
    return 1
}

# A DAG Airflow parsed whose file is then gone: it turns stale.
if ! airflow dags list -o plain 2>/dev/null | grep -q '^e2e_stale\b'; then
    cat >/opt/airflow/dags/e2e_stale.py <<'EOF'
import pendulum
from airflow import DAG
from airflow.decorators import task

with DAG("e2e_stale", start_date=pendulum.datetime(2026, 1, 1), schedule=None, tags=["e2e"]):

    @task
    def noop():
        pass

    noop()
EOF
    airflow dags reserialize >/dev/null 2>&1 || true
    parsed e2e_stale
fi
rm -f /opt/airflow/dags/e2e_stale.py

# Airflow 3 takes the DAG as an argument, 2 as -d; a refused form prints its
# usage to stdout, so only the form that worked is kept.
runs() {
    local out
    out=$(airflow dags list-runs -d "$1" -o plain 2>/dev/null) ||
        out=$(airflow dags list-runs "$1" -o plain)
    printf '%s\n' "$out"
}
trigger() {
    local dag=$1
    shift
    parsed "$dag"
    if [ -z "$(runs "$dag" | tail -n +2)" ]; then
        airflow dags trigger "$dag" "$@" >/dev/null
    fi
}
for dag in e2e_etl e2e_failing e2e_branch e2e_mapped e2e_retry e2e_running e2e_deferred e2e_paused; do
    trigger "$dag"
done
trigger e2e_params -c '{"day": "2026-09-28"}'
echo "seeded"
