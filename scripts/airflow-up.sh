#!/usr/bin/env bash
# Start the throwaway Airflow 2.9.3 that crates/cli/tests/live_airflow.rs
# runs against (scripts/airflow/compose.yaml), wait until its webserver
# answers, and seed it (scripts/airflow/seed.sh). It listens on
# http://127.0.0.1:18080 (AIRFLOW_PORT), user admin, password admin.
#
#   scripts/airflow-up.sh               Basic and session sign-in
#   AIRFLOW_AUTH=session scripts/airflow-up.sh   session only (2.x's default)
#   scripts/airflow-up.sh down          stop it and drop its volumes
#
# Then: AGENT_CLI_TEST_AIRFLOW=1 cargo test -p agent-cli --test live_airflow
set -euo pipefail
cd "$(dirname "$0")/airflow"

if [ "${1:-}" = down ]; then
    exec docker compose down -v
fi

case "${AIRFLOW_AUTH:-basic}" in
    basic) export AIRFLOW_AUTH_BACKENDS=airflow.api.auth.backend.basic_auth,airflow.api.auth.backend.session ;;
    session) export AIRFLOW_AUTH_BACKENDS=airflow.api.auth.backend.session ;;
    *) echo "airflow-up: AIRFLOW_AUTH is basic or session" >&2; exit 2 ;;
esac

docker compose up -d --wait airflow-webserver airflow-scheduler airflow-triggerer
docker compose exec -T airflow-scheduler bash /opt/airflow/e2e/seed.sh
echo "airflow-up: http://127.0.0.1:${AIRFLOW_PORT:-18080} (${AIRFLOW_AUTH:-basic})"
