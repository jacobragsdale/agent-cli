# lake in the contoso world

What the recordings in `http/lake.json` say, consistent with every other
domain's facts. `scripts/seed/lake.py` writes them and the Parquet files
under `files/lake/` together; change the tree there, run it, and add a line
here.

- Two accounts with a hierarchical namespace (the inventory, facts/arm.md):
  `contosolake` (rg-contoso-prod, Standard_ZRS, public network access
  Disabled, default action Deny, private endpoint `pe-contosolake-dfs`) and
  `contosolakedev` (rg-contoso-dev, open), whose one container `scratch` is
  empty. net has no recording of `pe-contosolake-dfs` yet.
- `contosolake` holds the zones `raw` and `curated` as containers and the
  data domains `sales` and `finance` as top-level folders in each. Four
  tables: `raw/sales/orders` (partitioned by `date`), `raw/finance/journal`,
  `curated/sales/orders_daily`, `curated/finance/ledger`.
- `raw/sales/orders` has `date=2026-09-23` to `date=2026-09-29`, one
  `part-0001.parquet` each (order_id, customer_id, total, placed_at), landed
  near 06:02 every day; `date=2026-09-27` is missing. `date=2026-09-28`
  holds orders 88121 to 88124, which `etl_nightly`'s `extract_orders` read
  (facts/airflow.md): 88123 has no customer_id. Today's, `date=2026-09-29`,
  landed at 06:02:11 with seven orders, 88125 to 88131, of which 88127 and
  88130 have no customer_id.
- `curated/sales/orders_daily` is one file `etl_nightly` last wrote at
  2026-09-27T00:38:10Z, a row a day for 2026-09-23 to 2026-09-26: the run
  that read the missing 2026-09-27 had nothing to add, and the 2026-09-29
  run failed on 88123, so raw against curated finds 2026-09-28 and
  2026-09-29 missing.
- `curated/finance/ledger` (entry_id, account, amount DECIMAL(12,2),
  posted) has 120 rows in one file, written at 2026-09-29T02:15:03Z by the
  finance close's service principal `…a2f2` (contoso-finance-close in
  facts/entra.md).
- ACLs: each container root grants other `--x`; `/sales` and below grant
  Airflow's identity `id-contoso-airflow` (principal `…a1f2`, facts/arm.md)
  `rwx`; `/finance` grants it `r-x`, and `/finance/ledger` names only
  `…a2f2`. It is in no group (Graph), and holds only Reader on
  rg-contoso-prod, no data role, so `lake access get
  contosolake/curated/finance/ledger --principal …a1f2` breaks at the
  ledger's folder (needs `r-x`, has `---`) and its next note's `arm role
  list` shows the Reader.
- Every Parquet file was written by DuckDB 1.4.1, so `path get` and `table
  get` read their rows and schemas from footers alone.
- `lake table diff` of `raw/sales/orders`'s 2026-09-28 and 2026-09-29 partitions finds one schema; raw orders against `curated/sales/orders_daily` share no column.
