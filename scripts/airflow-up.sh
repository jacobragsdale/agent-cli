#!/usr/bin/env bash
# Start the throwaway Airflow that crates/cli/tests/live_airflow.rs runs
# against (scripts/airflow/), wait until it answers, and seed it
# (scripts/airflow/seed.sh). User admin, password admin.
#
#   scripts/airflow-up.sh                        Airflow 2.9.3 on 127.0.0.1:18080,
#                                                Basic and session sign-in
#   AIRFLOW_AUTH=session scripts/airflow-up.sh   2.9.3 session-only (2.x's
#                                                default); restarts its webserver
#   AIRFLOW_VERSION=3 scripts/airflow-up.sh      Airflow 3.3.2 on 127.0.0.1:18081
#   scripts/airflow-up.sh down                   stop it and drop its volumes
#                                                (with AIRFLOW_VERSION=3: that one)
#
# Then: AGENT_CLI_TEST_AIRFLOW=1 cargo test -p agent-cli --test live_airflow
# (with AGENT_CLI_TEST_AIRFLOW_URL=http://127.0.0.1:18081 for Airflow 3).
set -euo pipefail
cd "$(dirname "$0")/airflow"

case "${AIRFLOW_VERSION:-2}" in
    2) compose=(docker compose -f compose.yaml) server=airflow-webserver port=18080
       services=(airflow-webserver airflow-scheduler airflow-triggerer) ;;
    3) compose=(docker compose -f compose-3.yaml) server=airflow-apiserver port=18081
       services=(airflow-apiserver airflow-scheduler airflow-dag-processor airflow-triggerer) ;;
    *) echo "airflow-up: AIRFLOW_VERSION is 2 or 3" >&2; exit 2 ;;
esac

if [ "${1:-}" = down ]; then
    exec "${compose[@]}" down -v
fi

case "${AIRFLOW_AUTH:-basic}" in
    basic) export AIRFLOW_AUTH_BACKENDS=airflow.api.auth.backend.basic_auth,airflow.api.auth.backend.session ;;
    session) export AIRFLOW_AUTH_BACKENDS=airflow.api.auth.backend.session ;;
    *) echo "airflow-up: AIRFLOW_AUTH is basic or session" >&2; exit 2 ;;
esac

"${compose[@]}" up -d --wait "${services[@]}"
"${compose[@]}" exec -T airflow-scheduler bash /opt/airflow/e2e/seed.sh
echo "airflow-up: http://127.0.0.1:${AIRFLOW_PORT:-$port} ($server, ${AIRFLOW_AUTH:-basic})"
