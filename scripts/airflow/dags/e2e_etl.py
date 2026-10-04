"""A run that succeeds and pushes XComs: a return value, a custom key and a large one."""
import pendulum
from airflow import DAG
from airflow.decorators import task

with DAG("e2e_etl", start_date=pendulum.datetime(2026, 1, 1), schedule=None, catchup=False,
         tags=["e2e", "etl"], description="Orders extract and load") as dag:

    @task
    def extract_orders(ti=None):
        ti.xcom_push(key="row_count", value=2)
        return [{"order_id": 88122, "customer_id": 4411, "paid": True, "note": None}]

    @task
    def big_payload():
        return ["x" * 100] * 400

    @task
    def load_orders(rows):
        return len(rows)

    load_orders(extract_orders()) >> big_payload()
