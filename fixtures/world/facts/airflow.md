# airflow in the contoso world

What the recordings in `http/airflow.json` say, consistent with every other
domain's facts. Keep a new recording consistent with these, and add a line
when you add a fact.

- Instance `prod` (`https://airflow.contoso.example`, Airflow 3.3.2, read_only; sign-in with any password) runs KubernetesExecutor task pods in `prod/web` and logs remotely to Azure Blob. DAGs `etl_nightly` (daily at 00:00), `orders_export` (hourly), `reports_weekly` (paused).
- Run `etl_nightly/scheduled__2026-09-29T00:00:00+00:00` **failed**: `extract_orders` and `transform_orders` succeeded, `load_orders` failed on try 2 with `ValueError: order 88123 has no customer_id` (`dags/etl_nightly.py:42`), `publish_report` is upstream_failed. Its pod `etl-nightly-load-orders-q8x1k2vz` (Failed, container `base`) is still in the cluster, with its log and events.
- `customer_sync` is missing from the DAG list: import error **12**, `ModuleNotFoundError: No module named 'contoso_crm'`.
- `etl_nightly.py` (DAG version 7, 61 lines) raises that `ValueError` at line 42, in `load_orders`; `prod`'s `dags_repo` is `airflow-dags:dags`, so `source get etl_nightly:42` prints `repo_file` `airflow-dags:dags/etl_nightly.py:42`. `load_orders` reads `extract_orders`' `return_value`: orders 88121 to 88124, and 88123 has no `customer_id`. `extract_orders` also pushed `extract_window`.
- Pools `default_pool` (128 slots, every task's) and `warehouse_writes` (2), nothing queued. Variables `orders_batch_size`, `orders_api_token` (encrypted) and `report_recipients`. Connections `airflow_logs` (wasb), `orders_api` (http, `orders.contoso.example`) and `reporting_dw` (mssql, `sql.contoso.example`, database `reporting`), whose `sql_conn` is the world config's `[[sql.connection]]` `reporting`; sql has no recordings.
