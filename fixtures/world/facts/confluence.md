# confluence in the contoso world

What the recordings in `http/confluence.json` say, consistent with every
other domain's facts. Keep a new recording consistent with these, and add a
line when you add a fact.

- Site `https://contoso.atlassian.net`; you are Jane Doe (`@me`, the token's account), with Sam Lee and Priya Patel, as in ado. Spaces `ENG` "Engineering" (id 2001, homepage **1000**) and `OPS` "Operations" (nothing recorded inside it).
- ENG's tree: 1000 "Engineering" → **1100** "Runbooks" (1101, 1102), **1200** "Release notes" (1201), **1300** "Orders search" and **1400** "Incidents" (empty: where a postmortem goes).
- **1101** "Runbook: etl_nightly" (labels `runbook`, `airflow`; version 4 by Sam Lee on 2026-09-25, created by Jane Doe): first checks, then the section "An order without customer_id": fix the order in the CRM and wait for the hourly customer sync, retry the load (clear `load_orders` for the failed run in Airflow), then re-run the search indexer `orders-sql` on `srch-contoso-prod`. Sam Lee's open inline comment **5002** on "retry the load" reads "only once the CRM fix has synced", so the page's `lossy` names the inline comment mark. Escalation mentions Sam Lee.
- **1102** "Runbook: worker crash loop" (label `runbook`, by Priya Patel, 2026-09-21): rotate `kv-contoso-prod/worker-db-password`, restart the worker in `prod/web`; it links ADO work item 1219 by URL.
- **1201** "Release notes v1.4.2" (version 3 by Jane Doe at 22:05 on 2026-09-28; version 2 by Sam Lee added the rollout time): links PR 431 and work items 1207 and 1210 as ADO URLs. Priya Patel's footer comment **5001** says the rollout looked clean. Attachments `att7001` `changes-v1.4.2.txt` (text, through the media redirect) and `att7002` `rollout.png` (binary: `--output`).
- **1300** "Orders search" (by Sam Lee): the AI Search design (index `orders` on `srch-contoso-prod`, indexer `orders-sql`), with a `toc` and a `jira` macro (SRCH-12), so its `lossy` is `toc`, `jira`.
- Searches recorded: "runbook etl_nightly" (1101, 1102), "orders search indexer" (1300, 1101), `--label runbook` (1101, 1102) and `--since 7d` (1201, 1101). `KEY:Title` lookups answer for "Runbook: etl_nightly" and "Release notes v1.4.2".
- Writes a trial may make answer and change nothing: a new page is 1401, an update of 1101 or 1102 is its next version, a footer comment is 5010, resolving 5002 answers resolved.
