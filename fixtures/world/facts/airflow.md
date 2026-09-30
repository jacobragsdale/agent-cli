# airflow in the contoso world

What the recordings in `http/airflow.json` say, consistent with every other
domain's facts. Keep a new recording consistent with these, and add a line
when you add a fact.

- Instance `prod` (`https://airflow.contoso.example`, Airflow 3.3.2, read_only; sign-in with any password) runs KubernetesExecutor task pods in `prod/web` and logs remotely to Azure Blob. DAGs `etl_nightly` (daily at 00:00), `orders_export` (hourly), `reports_weekly` (paused).
- Run `etl_nightly/scheduled__2026-09-29T00:00:00+00:00` **failed**: `extract_orders` and `transform_orders` succeeded, `load_orders` failed on try 2 with `ValueError: order 88123 has no customer_id` (`dags/etl_nightly.py:42`), `publish_report` is upstream_failed. Its pod `etl-nightly-load-orders-q8x1k2vz` (Failed, container `base`) is still in the cluster, with its log and events.
- `customer_sync` is missing from the DAG list: import error **12**, `ModuleNotFoundError: No module named 'contoso_crm'`.
