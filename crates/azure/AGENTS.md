# azure: kv, acr, aks, aisearch and arm

Five domains in one crate, `KV`, `ACR`, `AKS`, `AISEARCH`, `ARM`, over plain
HTTPS with `az` tokens; no Azure SDK. Docs: https://learn.microsoft.com/rest/api/
(keyvault, containerregistry, azureresourcegraph, searchservice, resources).

## Config
`[azure]`: `subscriptions`, `vaults`, `registries`, `search_services` (one
string or a list; empty means all the login reaches, else an allowlist that
sets row order), `refresh` (cache seconds, 300), `parallel` (8). No
credential of its own: `az_token(ctx, resource, fresh)` per audience.
Tokens go only to `management.azure.com`, `*.vault.azure.net`,
`*.azurecr.io`, `*.search.windows.net`, checked on every URL.

## Ids
- kv: `vault/name[/version]`, or the secret URI.
- acr: `loginserver/repo[:tag|@digest]`; a bare `repo:tag` finds its registry.
- aks: the cluster name; `k8s_scope` in its rows is the k8s scope id.
- aisearch: `service`, `service/index`, `service/index/key`,
  `service/indexer`; bare names with `--service`/`--index`; data-plane URLs
  and portal links (`aisearch/refs.rs`). Two holders of a bare name is exit 2.
- arm: the ARM id (a child collection too), a portal link, or a name with
  `--type`/`--group`/`--subscription`.

## Where things are (`src/`)
A command is `<domain>/<resource>/<verb>.rs` (`kv/secret/list.rs`).
- `lib.rs`: the five `Domain`s. `doctor.rs`: status and doctor of each.
  `config.rs`: `Azure::load`. `testing.rs`: shared scrubbed answers.
- `client.rs`: `bearer`, `ARM`, `VAULT`, `REGISTRY`, `SEARCH`, `narrow`
  (allowlists), `parallel`, `limited` (the `--limit` note), `stamp`, `text`.
- `graph.rs`: `rows` (any Resource Graph query, paged) and the inventory
  of vaults, registries, clusters and search services, cached.
- `kv/mod.rs`: `get` (the door), `holder`, `secret_ref`.
- `acr/mod.rs`: a `Session` per registry trades the token; `holder`.
- `aisearch/mod.rs`: `reach`, `holder`, and `Search`, the door: a token
  when the service takes roles, else an admin key from `listAdminKeys`;
  never both; 401/403 is exit 3 naming the role. `aisearch/shape.rs`:
  `bound` and `scrub` (definitions).
- `arm/mod.rs`: `subscriptions` (cached), `scope` (`--subscription`),
  `quote` (KQL), `scrub`. `arm/resource/get.rs`: ids, portal links, and
  api-versions (the provider's newest stable, cached).

## Fixtures
`fixtures/world/http/{azure,aisearch}.json`; facts in
`fixtures/world/facts/{kv,acr,aks,aisearch,arm}.md`; world checks
`crates/cli/tests/world_{kv,acr,aks,aisearch,arm}.rs`. Tests run
`scripts/fake/az` and `kubelogin` (the token is `token@<resource>`).

## Quirks
- An allowlist that reaches nothing is an error, never an empty answer.
- Only `kv secret get` (a `Reveal`) returns a value.
- ACR wants a `containerregistry.azure.net` token, not ARM's.
- AI Search: `status: running` is health (`ok`), not a run; a 429 is the
  tier's quota; data plane `2026-04-01`, ARM `2025-05-01`, no previews.

## Never needed
Other crates' sources, `docs/plans/`, `docs/reference/`.
