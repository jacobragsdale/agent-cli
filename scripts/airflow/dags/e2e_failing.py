"""A run that fails with a traceback in this file, and a task left upstream_failed."""
import pendulum
from airflow import DAG
from airflow.decorators import task


def check(row):
    if "customer_id" not in row:
        raise ValueError(f"order {row['order_id']} has no customer_id")


with DAG("e2e_failing", start_date=pendulum.datetime(2026, 1, 1), schedule=None, catchup=False,
         tags=["e2e"], default_args={"retries": 0}) as dag:

    @task
    def extract_orders():
        return [{"order_id": 88123}]

    @task
    def load_orders(rows):
        for row in rows:
            check(row)

    @task
    def publish_report():
        pass

    load_orders(extract_orders()) >> publish_report()
