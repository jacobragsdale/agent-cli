"""A task that fails and waits an hour for its next try: up_for_retry."""
from datetime import timedelta

import pendulum
from airflow import DAG
from airflow.decorators import task

with DAG("e2e_retry", start_date=pendulum.datetime(2026, 1, 1), schedule=None, catchup=False,
         tags=["e2e"], default_args={"retries": 3, "retry_delay": timedelta(hours=1)}) as dag:

    @task
    def flaky():
        raise ConnectionError("warehouse refused the connection")

    flaky()
