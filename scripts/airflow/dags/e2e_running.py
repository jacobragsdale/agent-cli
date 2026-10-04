"""A run that keeps running: one task sleeps, one waits on a pool with no slots."""
import time

import pendulum
from airflow import DAG
from airflow.decorators import task

with DAG("e2e_running", start_date=pendulum.datetime(2026, 1, 1), schedule=None, catchup=False,
         tags=["e2e"]) as dag:

    @task
    def sleep():
        time.sleep(6 * 3600)

    @task(pool="e2e_zero")
    def starved():
        pass

    sleep()
    starved()
