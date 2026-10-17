# azure: kv, acr, aks, aisearch, arm, net, lake and entra

Eight domains, `KV`, `ACR`, `AKS`, `AISEARCH`, `ARM`, `NET`, `LAKE`,
`ENTRA`, over plain HTTPS with `az` tokens; no Azure SDK. A curated command
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
- `client.rs`: `bearer`, the audiences, `claims`, `guid`, `narrow`,
  `parallel`, `limited`, `stamp`, `text`, `decode`, `arm_only`, `settle`.
- `graph.rs`: `rows` (any Resource Graph query, paged); the cached
  inventory of vaults, registries, clusters, search services, lakes.
- `kv/mod.rs`: ids `vault/name[/version]`; `get`, `found`, `put`.
- `acr/mod.rs`: ids `loginserver/repo[:tag|@digest]`; `Session`.
- `aks/mod.rs`: a cluster's name, ARM id or portal link; `locate`.
- `aisearch/mod.rs`: `Search`, the door; ids in `aisearch/refs.rs`.
- `arm/mod.rs`: `subscriptions`, `scope`, `quote`, `of_type`, `scrub`.
  `arm/resource/mod.rs`: `resolve`, `any`. `arm/role/mod.rs`: `assignments`.
  `arm/identity/mod.rs`: `federated` (a subject as a serviceaccount id).
- `net/mod.rs`: `Place`, `read`, `found`, `leaf`, `usages`, `rule`.
- `lake/mod.rs`: ids `account/container/path` (`Ref`, also abfss, wasbs,
  https and azure URLs); `Lake`, the door. `lake/parquet.rs`: `footers`.
  `lake/table/mod.rs`: tables, partitions and `bulk` (OPENROWSET's path).
- `entra/mod.rs`: `Graph`, the door (`get`, `query`, `pages`, `by_ids`);
  `find` reads every id a get takes; `credentials`, `owners`, `resource`.

## Fixtures
`fixtures/world/http/{azure,aisearch,lake,entra}.json`;
`scripts/seed/lake.py` writes lake's and the Parquet files it serves.
Facts in `fixtures/world/facts/<domain>.md`; world checks
`crates/cli/tests/world_<domain>.rs`. Tests run `scripts/fake/az`,
`kubelogin` (token: `token@<resource>`).

## Quirks
- Only `kv secret get` (a `Reveal`) returns a value; entra never asks
  Graph for a secret or certificate's bytes.
- `VERIFY(work)` marks API behaviour no live call has confirmed yet.

## Never needed
Other crates' sources, `docs/reference/`.
