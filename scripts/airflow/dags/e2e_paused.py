"""A paused DAG: its triggered run stays queued."""
import pendulum
from airflow import DAG
from airflow.decorators import task

with DAG("e2e_paused", start_date=pendulum.datetime(2026, 1, 1), schedule="0 0 * * *", catchup=False,
         tags=["e2e"], is_paused_upon_creation=True) as dag:

    @task
    def noop():
        pass

    noop()
