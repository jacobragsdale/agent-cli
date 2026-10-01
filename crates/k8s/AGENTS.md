# k8s: Kubernetes through kubectl

A thin, bounded, JSON-emitting `kubectl` wrapper, not a kubectl replacement:
https://kubernetes.io/docs/reference/kubectl/

## Config
`[[k8s.scope]]`: `name` (what `--cluster` takes), `context` (the kube
context; defaults to `name`), `namespaces` (one or a list; empty lists every
namespace). No credential of its own: kubectl's kubeconfig (and `kubelogin`
for AKS) signs in.

## Ids
`cluster/namespace/name`, where `cluster` is the scope name (`prod/web/api`).
Every one-object verb also takes `namespace/name` or a name, with
`--cluster` and `--namespace` filling in the rest; a ref and a flag that
disagree is exit 2. `dd` and `airflow` rows print pods in this form.

## Where things are (`src/`)
A command is `<resource>/<verb>.rs`: its args, rows, handler, `command!`
and tests (`deployment/list.rs` is `k8s deployment list`). Copy a sibling.
- `lib.rs`: `K8S`, whose `commands` registers every command (its order is
  the listing's). `doctor.rs`: status and doctor.
- `kubectl.rs`, what every command shares:
  - `At` (the `--cluster`, `--namespace` args, flattened into each command's
    args): `listing(ctx)` for a list, `one(ctx)` for one object,
    `named(ctx, raw)` for an id. Scopes default through core's `pick`.
  - `Target`: `read(ctx, args)`, `json(ctx, args)`, `write(ctx, args)`
    (always `Destructive`), `id(item)`, `row_namespace(item)`; `kubectl(args)`
    and `finished` for a caller that reads kubectl's failure itself
    (`deployment wait`).
  - `items`, `age`, `limited` (the `--limit` note), `digest`, `owner_of`,
    `kubectl_error`.
- `<resource>/mod.rs`: what its verbs share (`pod/mod.rs`: `PodRow`, `row`,
  `containers`, the SecretProviderClass reading, `previous_logs`, the note
  a crash loop prints; `deployment/mod.rs`: `DeploymentRow`, `row`).
- `testing.rs`: `k8s(argv)`, `k8s_with(argv, config)`, `SCOPES` for tests.

## Fixtures
Tests run `scripts/fake/kubectl` (a Python stand-in: contexts `aks-qa` and
`aks-prod` built in; `FAKE_KUBECTL_LOG` records calls). Trials read
`fixtures/world/kubectl.json` instead. Facts `fixtures/world/facts/k8s.md`;
world checks `crates/cli/tests/world_k8s.rs`; queries `crates/k8s/search.toml`.

## Quirks
- Every call is `kubectl --context C --request-timeout=10s …` under the
  command's deadline; the process group is killed at it, so a kubelogin
  device-code prompt cannot hang.
- Tests see only what `scripts/fake/kubectl` understands (it reads `-l`
  selectors already); a new kubectl flag may need teaching it there.
- `secret list` rows never carry values; only `secret get` (a `Reveal`)
  does. `pod get` lists `secret_refs` with the Key Vault ids (`kv`) a
  SecretProviderClass maps.

## Never needed
Other crates' sources, `docs/plans/`, `docs/reference/`.
