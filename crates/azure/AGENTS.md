# azure: kv, acr, aks, aisearch, arm, net and lake

Seven domains, `KV`, `ACR`, `AKS`, `AISEARCH`, `ARM`, `NET`, `LAKE`, over
plain HTTPS with `az` tokens; no Azure SDK. `arm resource` and `arm query run` read any resource; a curated command
adds a join or a bound. Never calls k8s: joins are ids one side prints.

## Config
`[azure]`: `subscriptions`, `vaults`, `registries`, `search_services`,
`storage_accounts` (a string or list; empty is all the login reaches, else
an allowlist in row order), `refresh` (cache seconds, 300), `parallel` (8).
No credential: `az_token(ctx, resource, fresh)` per audience. Tokens go only
to `management.azure.com`, `*.vault.azure.net`, `*.azurecr.io`,
`*.search.windows.net`, an account in reach's `*.dfs.core.windows.net`,
`graph.microsoft.com`, checked on every URL. An allowlist that reaches
nothing is an error, never an empty answer.

## Where things are (`src/`)
A command is `<domain>/<resource>/<verb>.rs`. Each domain's module doc
(`kv/mod.rs` …) gives its ids, helpers and quirks.
- `lib.rs`: the `Domain`s. `doctor.rs`, `config.rs` (`Azure::load`),
  `testing.rs` (shared scrubbed answers).
- `client.rs`: `bearer`, the audiences, `narrow` (allowlists), `parallel`,
  `limited`, `stamp`, `text`, `decode`, `arm_only`, `settle` (a 202 polled).
- `graph.rs`: `rows` (any Resource Graph query, paged); the cached
  inventory of vaults, registries, clusters, search services, lakes.
- `kv/mod.rs`: ids `vault/name[/version]`; `get`, `found`, `put`;
  `item_ref`, `holder`, `across`.
- `acr/mod.rs`: ids `loginserver/repo[:tag|@digest]`; `Session`; runs
  `registry/runid`.
- `aks/mod.rs`: a cluster's name, ARM id or portal link; `locate`.
- `aisearch/mod.rs`: `Search`, the door; ids in `aisearch/refs.rs`.
- `arm/mod.rs`: `subscriptions`, `scope`, `quote`, `of_type`, `scrub`.
  `arm/resource/mod.rs`: `resolve`, `any`. `arm/role/mod.rs`: `assignments`.
- `net/mod.rs`: `Place`, `read`, `found`, `leaf`, `usages`, `rule`.
- `lake/mod.rs`: ids `account/container/path` (`Ref`, which also reads
  abfss, wasbs, https and azure URLs); `Lake`, the door (`list`, `head`,
  `range`, `containers`). `lake/parquet.rs`: `footers` by a ranged read and
  a Thrift decoder of the fields wanted. `lake/table/mod.rs`: tables,
  partitions and `bulk` (the path Synapse's OPENROWSET takes).

## Fixtures
`fixtures/world/http/{azure,aisearch,lake}.json`; `scripts/seed/lake.py`
writes the lake recordings and the Parquet files it serves. Facts in
`fixtures/world/facts/{kv,acr,aks,aisearch,arm,net,lake}.md`; world checks
`crates/cli/tests/world_{kv,acr,aks,aisearch,arm,net,lake}.rs`. Tests run
`scripts/fake/az`, `kubelogin` (token: `token@<resource>`).

## Quirks
- Only `kv secret get` (a `Reveal`) returns a value.
- `VERIFY(work)` marks API behaviour no live call has confirmed yet.

## Never needed
Other crates' sources, `docs/reference/`.
