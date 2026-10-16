# k8s: Kubernetes through kubectl

A thin, bounded, JSON-emitting `kubectl` wrapper, not a kubectl replacement:
https://kubernetes.io/docs/reference/kubectl/

## Config
`[[k8s.scope]]`: `name` (what `--cluster` takes), `context` (the kube
context; defaults to `name`), `namespaces` (one or a list; empty lists every
namespace). No credential of its own: kubectl's kubeconfig (and `kubelogin`
for AKS) signs in.

## Ids
`cluster/namespace/name`, where `cluster` is the scope name (`prod/web/api`);
`cluster/name` for a cluster-scoped kind (node, pv). Every one-object verb
also takes `namespace/name` or a name, with `--cluster` and `--namespace`
filling in the rest; a ref and a flag that disagree is exit 2.

## Where things are (`src/`)
A command is `<resource>/<verb>.rs`: its args, rows, handler, `command!`
and tests. Copy a sibling.
- `lib.rs`: `K8S`, whose `commands` is the listing's order. `doctor.rs`.
- `kubectl.rs`, what every command shares:
  - `At` (`--cluster`, `--namespace`, flattened into each command's args):
    `listing(ctx)`, `one(ctx)`, `named(ctx, raw)`; for cluster-scoped kinds
    `cluster_wide(ctx)`, `named_cluster(ctx, raw)`; `cluster_of`.
  - `Target`: `read`, `json`, `write(ctx, effect, args)`, `id(item)`,
    `row_namespace(item)`; `kubectl(args)` and `finished` (`deployment wait`).
  - `items`, `age`, `limited`, `name_has`, `digest`,
    `owner_of` (a Job named for its schedule is its CronJob), `owned_by`.
- `shape.rs`: `events_for` (five newest, by uid), `events_in` (from a
  joined read), `newest`, `event_row`, `conditions`, `trim`, `listing_for`
  (the not-found hint), `selects`, `selector_text`.
- `kind/mod.rs`: `kinds` (api-resources, cached), `kind_named`.
- `<resource>/mod.rs`: what its verbs share (`pod/mod.rs`: `PodRow`, `row`,
  `containers`, `status_word`, `previous_logs`, `ready`, `first_unready`,
  a template's `probes`, `sized`, `volumes`, `images`; `service/mod.rs`:
  `endpoints`; `pv/mod.rs`: `row` with the disk's ARM id, `attached_to`;
  `daemonset/mod.rs` reads kube-system whatever the scope names).
- `testing.rs`: `k8s(argv)`, `k8s_with(argv, config)`, `SCOPES` for tests.

## Fixtures
Tests run `scripts/fake/kubectl` over `fixtures/cluster.json` (contexts
`aks-qa`, `aks-prod`; a new kind is data there, `server` the version and
failing readyz checks; `FAKE_KUBECTL_LOG` records calls). Trials read `fixtures/world/kubectl.json`. Facts
`fixtures/world/facts/k8s.md`; world checks `crates/cli/tests/world_k8s.rs`;
queries `crates/k8s/search.toml`.

## Quirks
- Every call is `kubectl --context C --request-timeout=10s …`, its process
  group killed at the deadline: a kubelogin prompt cannot hang.
- A new kubectl flag may need teaching `scripts/fake/kubectl` (it reads
  `-l`, `--field-selector` and `api-resources`).
- Only `secret get` (a `Reveal`) prints values. `pod get` lists `secret_refs` with the Key Vault ids (`kv`) a
  SecretProviderClass maps. `ingress get` checks a TLS secret by `-o name`.

## Never needed
Other crates' sources, `docs/plans/`, `docs/reference/`.
