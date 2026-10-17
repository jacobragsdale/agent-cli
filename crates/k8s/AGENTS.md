# k8s: Kubernetes through kubectl

A bounded `kubectl` wrapper: `k8s object` reads any kind; a curated command
adds a join or a bound. Never calls azure: joins are ids it prints.

## Config
`[[k8s.scope]]`: `name` (what `--cluster` takes), `context` (the kube
context; defaults to `name`), `namespaces` (one or a list; empty lists every
namespace). kubectl's kubeconfig (and `kubelogin` for AKS) signs in.

## Ids
`cluster/namespace/name`, where `cluster` is the scope name (`prod/web/api`);
`cluster/name` for a cluster-scoped kind (node, pv). Every one-object verb
also takes `namespace/name` or a name, with `--cluster` and `--namespace`
filling in the rest; a ref and a flag that disagree is exit 2.

## Where things are (`src/`)
A command is `<resource>/<verb>.rs`; copy a sibling.
- `lib.rs`: `K8S` (commands in listing order). `doctor.rs`.
- `kubectl.rs`, what every command shares:
  - `At` (`--cluster`, `--namespace`, flattened):
    `listing(ctx)`, `one(ctx)`, `named(ctx, raw)`; for cluster-scoped kinds
    `cluster_wide(ctx)`, `named_cluster(ctx, raw)`; `cluster_of`.
  - `Target`: `read`, `json`, `write(ctx, effect, args)`, `id(item)`,
    `row_namespace(item)`; `kubectl(args)`, `finished`.
  - `items`, `age`, `limited`, `name_has`, `digest`, `Changed`,
    `owner_of` (a Job named for its schedule is its CronJob), `owned_by`.
- `diff.rs`: `Sides` (`read`, `workloads`, `manifests`), `RULES`,
  `workload`, `clean`, `inline_secrets`, `next_secret`.
- `shape.rs`: `events_for` (five newest, by uid), `events_in` (from a
  joined read), `newest`, `event_row`, `conditions`, `trim`, `listing_for`
  (the not-found hint), `selects`, `selector_text`.
- `kind/mod.rs`: `kinds` (api-resources, cached), `kind_named`.
- `<resource>/mod.rs`: what its verbs share (`pod/mod.rs`: `PodRow`, `row`,
  `containers`, `status_word`, `previous_logs`, `ready`, `first_unready`,
  a template's `probes`, `sized`, `volumes`, `images`; `pv/mod.rs`: `row`,
  `attached_to`; `node/mod.rs`: `row`, `top` (metrics-server, else a
  note), `requests`, `quantity`; `access/mod.rs`: `rules` (can-i --list);
  `deployment/mod.rs`: `revisions`; `daemonset/mod.rs` reads kube-system).
- `testing.rs`: `k8s(argv)`, `k8s_with(argv, config)`, `SCOPES`,
  `dry_run(argv)`.

## Fixtures
Tests run `scripts/fake/kubectl` over `fixtures/cluster.json` (contexts
`aks-qa`, `aks-prod`; a kind is data; `server`, `top`, `access`, `exec`
answer version, metrics, can-i, `pod run`). Trials read
`fixtures/world/kubectl.json` (scopes `prod`, `staging`); facts
`fixtures/world/facts/k8s.md`; checks `crates/cli/tests/world_k8s.rs`.

## Quirks
- Every call is `kubectl --context C --request-timeout=10s …`, killed at
  the deadline.
- A new kubectl flag may need teaching `scripts/fake/kubectl`.
- Node events carry the name as uid: `node get` matches kind and name.
- Only `secret get` (a `Reveal`) prints values; `pod run` masks those named
  like secrets. `pod get` lists `secret_refs` with Key Vault ids (`kv`).

## Never needed
Other crates' sources, `docs/reference/`.
