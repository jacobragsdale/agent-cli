# azure: kv, acr and aks

Three domains in one crate, `KV`, `ACR`, `AKS`, over plain HTTPS with `az`
tokens; no Azure SDK. Key Vault https://learn.microsoft.com/rest/api/keyvault/,
ACR https://learn.microsoft.com/rest/api/containerregistry/, Resource Graph
https://learn.microsoft.com/rest/api/azureresourcegraph/.

## Config
`[azure]`: `subscriptions`, `vaults`, `registries` (each one string or a
list; empty means everything the login reaches, non-empty is an allowlist
that also sets row order), `refresh` (cache seconds, default 300),
`parallel` (default 8). No credential of its own: `az_token(ctx, resource,
fresh)` per audience. Tokens go only to `management.azure.com`,
`*.vault.azure.net` and `*.azurecr.io`, checked on every URL (`nextLink`s
and cached addresses too).

## Ids
- kv: `vault/name`, `vault/name/version`, or the secret URI.
- acr: `loginserver/repo`, `loginserver/repo:tag` (or `@digest`); a bare
  `repo:tag` finds its registry, and two holders is exit 2.
- aks: the cluster name; `k8s_scope` in its rows is the k8s scope id.

## Where things are
- `lib.rs`: `KV`, `ACR`, `AKS`, whose `commands` register every command
  (their order is the listing's), status and doctor, and shared helpers:
  `Azure::load`, `az_token`, `bearer` (a `Mint`), `allowed`, `missing`,
  `narrow` (allowlists), `parallel` (bounded fan-out), `limited` (a list's
  `--limit` note), `stamp`, `from_unix`, `text`, `refused_with`.
- `graph.rs`: one Resource Graph query for every vault, registry and
  cluster, cached for `refresh`.
- `kv.rs`: vault, secret, version; `get(ctx, vault, url)` and `pages` are
  the door, `holder` finds which vault has a name.
- `acr.rs`: registry, repo, tag, manifest; a `Session` per registry
  exchanges the token and does `get`; `holder`, `image_ref`.
- `aks.rs`: cluster list and connect (runs `az aks get-credentials` and
  `kubelogin`).

## Fixtures
`fixtures/world/http/azure.json`; facts `fixtures/world/facts/{kv,acr,aks}.md`;
world checks `crates/cli/tests/world_{kv,acr,aks}.rs`; queries
`crates/azure/search.toml`. Tests run `scripts/fake/az` and `kubelogin`
(the token is `token@<resource>`).

## Quirks
- An allowlist that reaches nothing is an error, never an empty answer.
- A name found in two vaults or registries is exit 2, never a guess.
- Only `kv secret get` (a `Reveal`) returns a value; no row type has a field
  for one.
- ACR wants a `containerregistry.azure.net` token, not ARM's: hardened
  registries refuse ARM-scoped ones.

## Never needed
Other crates' sources, `PLAN.md`, `docs/plans/`, `docs/reference/`.
