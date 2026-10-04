"""A branch that skips one side."""
import pendulum
from airflow import DAG
from airflow.decorators import task

with DAG("e2e_branch", start_date=pendulum.datetime(2026, 1, 1), schedule=None, catchup=False,
         tags=["e2e"]) as dag:

    @task.branch
    def choose():
        return "left"

    @task
    def left():
        pass

    @task
    def right():
        pass

    @task(trigger_rule="none_failed")
    def join():
        pass

    choose() >> [left(), right()] >> join()
