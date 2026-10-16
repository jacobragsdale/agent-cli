# azure: kv, acr, aks, aisearch, arm and net

Six domains, `KV`, `ACR`, `AKS`, `AISEARCH`, `ARM`, `NET`, over plain HTTPS
with `az` tokens; no Azure SDK. Docs: https://learn.microsoft.com/rest/api/
(keyvault, containerregistry, azureresourcegraph, searchservice, resources,
virtualnetwork, monitor, authorization, managedidentity).

## Config
`[azure]`: `subscriptions`, `vaults`, `registries`, `search_services` (a
string or list; empty is all the login reaches, else an allowlist in row
order), `refresh` (cache seconds, 300), `parallel` (8). No credential of
its own: `az_token(ctx, resource, fresh)` per audience. Tokens go only to
`management.azure.com`, `*.vault.azure.net`, `*.azurecr.io`,
`*.search.windows.net`, checked on every URL. An allowlist that reaches
nothing is an error, never an empty answer.

## Where things are (`src/`)
A command is `<domain>/<resource>/<verb>.rs` (`kv/secret/list.rs`). Each
domain's module doc (`kv/mod.rs` …) gives its ids, helpers and quirks.
- `lib.rs`: the `Domain`s. `doctor.rs`: their status and doctor.
  `config.rs`: `Azure::load`. `testing.rs`: shared scrubbed answers.
- `client.rs`: `bearer`, `ARM`, `VAULT`, `REGISTRY`, `SEARCH`, `narrow`
  (allowlists), `parallel`, `limited` (the `--limit` note), `stamp`,
  `text`, `decode` (portal links), `arm_only`, `settle` (a 202's Location
  polled under the deadline).
- `graph.rs`: `rows` (any Resource Graph query, paged); the cached
  inventory of vaults, registries, clusters, search services.
- `kv/mod.rs`: ids `vault/name[/version]`; `get`, the door.
- `acr/mod.rs`: ids `loginserver/repo[:tag|@digest]`; `Session`.
- `aks/mod.rs`: a cluster's name, ARM id or portal link, a pool
  `cluster/pool`; `locate`, `Located`. `aks/nodepool/mod.rs`: `pool`.
- `aisearch/mod.rs`: `Search`, the door; ids in `aisearch/refs.rs`.
- `arm/mod.rs`: `subscriptions`, `scope` (`--subscription`), `quote`
  and `of_type` (KQL), `scrub`. `arm/resource/mod.rs`: `resolve`, any ARM
  ref (an id, a portal link, a name within a `Lookup`), for arm and net;
  `any` for a `--resource` flag. `arm/role/mod.rs`: `assignments`.
- `net/mod.rs`: `Place` (`--group`, `--subscription`; `rows`, any Resource
  Graph tail), `read`, `found` (a 404 is `None`), `leaf`, `usages`, `rule`
  (an NSG rule as written or in effect). `net/ip/mod.rs`: `public_ips`,
  each with its k8s service.

## Fixtures
`fixtures/world/http/{azure,aisearch}.json`; facts in
`fixtures/world/facts/{kv,acr,aks,aisearch,arm,net}.md`; world checks
`crates/cli/tests/world_{kv,acr,aks,aisearch,arm,net}.rs`. Tests run
`scripts/fake/az`, `kubelogin` (token: `token@<resource>`).

## Quirks
- Only `kv secret get` (a `Reveal`) returns a value.
- `VERIFY(work)` marks API behaviour no live call has confirmed yet.

## Never needed
Other crates' sources, `docs/plans/`, `docs/reference/`.
