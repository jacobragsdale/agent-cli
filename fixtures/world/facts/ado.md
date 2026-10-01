# ado in the contoso world

What the recordings in `http/ado.json` say, consistent with every other
domain's facts. Keep a new recording consistent with these, and add a line
when you add a fact.

- Org `contoso`, project `Fabrikam`, team `Fabrikam Team`, repos `api`, `worker` and `airflow-dags` (each on `main`). You are Jane Doe (`@me`).
- PR **431** "Retry on 429 from the orders service" (reviewers Sam Lee and Priya Patel, both approved) merged commit `4be1c0d2…` into `main`; it closes work items **1207** and **1210**.
- Git tag `v1.4.2` on that commit triggered `api-ci` run **8809**, which **failed** in "Run tests" (`OrdersClientTests.RetriesOn429` timed out; `ado run logs 8809`); the re-run **8812** succeeded and pushed the image.
- Assigned to `@me` in Sprint 42: **1218** (Bug, New, priority 1: the worker crash loop), **1215** (Task, Active: retry jitter, PR 436 open), **1207** (Resolved).
- No open pull request waits on your review (`ado pr list --vote none`); Sam Lee has not voted on 436. You queued runs 8812, 8809 and 8801 (`--requested-by @me`); Sam Lee queued 8811 by hand.
- A pending approval: `db-migrations` run 8811, "Apply migration 0042 to prod".
- PR **436** (head `a7e3c9f1…`, based on `main` at `4be1c0d2…`, closes 1215) has one unresolved thread, **436/7**: Sam Lee on `src/Orders/OrderClient.cs:42`, the jitter line, asking to cap the delay at 30 s, with a +1 from Priya Patel. 436/6 is fixed; its reviewer policy is queued.
- Code Search for `IOrderClient` finds it in `api` (`src/Orders/IOrderClient.cs:6` declares it, `src/Orders/OrderClient.cs:11` implements it) and `worker` (`src/Jobs/Retry.cs:18` calls it). `airflow-dags:dags/etl_nightly.py` is the DAG prod Airflow runs; its line 42 raises `ValueError(f"order {order_id} has no customer_id")`.
- Tags `v1.4.1` (`2d8b6f4a…`) and `v1.4.2` (`4be1c0d2…`, PR 431's merge): `api@v1.4.1..v1.4.2` changes `src/Orders/OrderClient.cs` (retry on 429) and `tests/Api.Tests/OrdersClientTests.cs` (`RetriesOn429` among the tests added). PR 436 changes the same two files from `4be1c0d2…`.
