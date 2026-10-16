# acr in the contoso world

What the recordings in `http/azure.json` say, consistent with every other
domain's facts. Keep a new recording consistent with these, and add a line
when you add a fact.

- Registry `contosoacr` (`contosoacr.azurecr.io`): `api` tags `v1.4.2`, `v1.4.1`, `v1.4.0`; `worker` tags `v1.4.2`, `v1.4.1`; `reports` tag `v0.9.3`. The `v1.4.2` digests match what the pods run.
- Repository `reports`: tag `v0.9.3` (pushed 2026-09-26T14:10:11Z), what `k8s deployment get prod/web/reports-api` prints.
- `contosoacr` (`acr registry get`) is Standard: admin user off, public network access Enabled, no network rules, replications or private endpoints (those are Premium's), retention off, no webhooks.
- ACR Tasks runs (`acr run list`, newest first, all `az acr build` quick runs): `ca1g` **failed** 2026-09-29 09:12:03 to 09:12:44 (`failed during run, err: exit status 1`), building reports v0.9.4, whose `dotnet test` step failed `MonthlyReportTests.TotalsMatchOrders` (412 expected, 411 found); `acr run get contosoacr/ca1g` notes `acr run logs contosoacr/ca1g`, whose log says so. `ca1f` built `worker:v1.4.2` and `ca1e` `api:v1.4.2` for api-ci run 8812 on 2026-09-28 (21:00:52 to 21:02:41, the tag's push time), the image the api pods run; `ca1d` built `reports:v0.9.3`. The log's SAS link is never printed.
