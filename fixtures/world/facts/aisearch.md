# aisearch in the contoso world

What the recordings in `http/aisearch.json` (and the two Resource Graph rows
in `http/azure.json`) say, consistent with every other domain's facts. Keep a
new recording consistent with these, and add a line when you add a fact.

- Service `srch-contoso-prod` (standard, 2 replicas, `rg-contoso-prod`, westeurope, semantic ranker on the standard plan) takes roles and keys (`aadOrApiKey`), so the CLI sends an `az` token. Service `srch-contoso-dev` (basic, `rg-contoso-dev`) takes keys only (`apiKeyOnly`), so every call carries the admin key ARM's `listAdminKeys` hands out (`fixture-admin-key-dev-primary`, never printed).
- Index `srch-contoso-prod/orders`: 88,120 documents, key `id`; `customer_id` and `status` filterable (`status` facetable), `total` and `created` sortable, `summary` searchable (`en.microsoft`), and `summary_vector`, 1,536 dimensions, retrievable, profile `orders-hnsw` with vectorizer `aoai-embed` (Azure OpenAI `text-embedding-3-small`). Default semantic configuration `orders-semantic` (content `summary`, keywords `status`). Its etag is `"0x8DCE0A1B2C3D4E5"`.
- Documents `88121` (shipped), `88122` (delayed: late delivery) and `88124` (shipped) are in it; any search answers those three, 88122 first, out of 88,120. Document **88123 is not**: a lookup is exit 4, and its hint names `indexer list --failing`.
- Indexer `srch-contoso-prod/orders-sql` reads data source `orders-sql` (`azuresql`, table `dbo.orders`, high-water mark on `modified`) into `orders` daily at 00:40 (`P1D`), after `etl_nightly`. Its health is `ok` (the service says `running`). Its last run, 2026-09-29T00:40:00Z to 00:41:12Z, **succeeded with 1 failed item of 412**: key `88123`, "Column 'customer_id' of row 88123 is null", the same order airflow's `etl_nightly` `load_orders` failed on. `indexer get` notes `document get srch-contoso-prod/orders/88123`. The three runs before it had no failures.
- Index `srch-contoso-dev/products` (key `sku`, 1,200 documents, no vectors) has no indexer.
