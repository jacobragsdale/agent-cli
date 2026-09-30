# ado in the contoso world

What the recordings in `http/ado.json` say, consistent with every other
domain's facts. Keep a new recording consistent with these, and add a line
when you add a fact.

- Org `contoso`, project `Fabrikam`, team `Fabrikam Team`, repo `api`. You are Jane Doe (`@me`).
- PR **431** "Retry on 429 from the orders service" (reviewers Sam Lee and Priya Patel, both approved) merged commit `4be1c0d2…` into `main`; it closes work items **1207** and **1210**.
- Git tag `v1.4.2` on that commit triggered `api-ci` run **8809**, which **failed** in "Run tests" (`OrdersClientTests.RetriesOn429` timed out; `ado run logs 8809`); the re-run **8812** succeeded and pushed the image.
- Assigned to `@me` in Sprint 42: **1218** (Bug, New, priority 1: the worker crash loop), **1215** (Task, Active: retry jitter, PR 436 open), **1207** (Resolved).
- No open pull request waits on your review (`ado pr list --vote none`); Sam Lee has not voted on 436. You queued runs 8812, 8809 and 8801 (`--requested-by @me`); Sam Lee queued 8811 by hand.
- A pending approval: `db-migrations` run 8811, "Apply migration 0042 to prod".
