"""A mapped task whose index 1 fails."""
import pendulum
from airflow import DAG
from airflow.decorators import task

with DAG("e2e_mapped", start_date=pendulum.datetime(2026, 1, 1), schedule=None, catchup=False,
         tags=["e2e"], default_args={"retries": 0}) as dag:

    @task
    def double(x):
        if x == 2:
            raise ValueError(f"cannot double {x}")
        return x * 2

    double.expand(x=[1, 2, 3])
