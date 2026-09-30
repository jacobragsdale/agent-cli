# First live run: kv, acr, aks, k8s

The four domains were built and tested without Azure or a cluster: fixtures
stand in for Resource Graph, Key Vault and the registries, and
`scripts/fake/kubectl` for kubectl. az-tui, where the code came from, never
had a live run either. This list is that run. Work through it in order on a
machine that reaches the subscription and the clusters.

**Before you paste anything into an issue or a commit, scrub it.** Vault,
registry, cluster, subscription and tenant names never go into this public
repository. Replace them with `contoso`-style placeholders.

## What to capture when a step fails

- The exact command line and its exit code (`echo $?`).
- stderr in full. It is already redacted, but check it for names anyway.
- For HTTP failures: the status and the service's message. Both are in the
  error line.
- For kubectl failures: the same command run by hand with `-v=6`, trimmed to
  the request line and the response status.
- Anything slow, with `time` in front of the command.

## Azure (kv, acr, aks)

1. **Log in.** Run `az login`. `az account list -o table` should show the
   subscriptions you expect. Put `[azure]` in `~/.config/agent-cli/config.toml`,
   even an empty one, so doctor checks it.
2. **Doctor.** Run `agent-cli doctor kv`, `agent-cli doctor acr` and
   `agent-cli doctor aks`.
   - Expect `az login` plus the vault and registry token rows to be ok, each
     under a second or so. If the `az` token call is over about 300 ms, note
     the number: it decides the disk token cache in TODO.md.
   - Expect the inventory counts to match the portal.
   - Expect each allowlisted vault or registry to answer. A name that is not
     found is reported as such.
3. **Inventory.**
   - `agent-cli kv vault list`, `agent-cli acr registry list` and
     `agent-cli aks cluster list` should list the same resources the portal
     does.
   - `kubernetes_version` should be the running version, and `power_state`
     should be `Running` or `Stopped`.
   - Run the command again: it should come back at once from the cache. With
     `--no-cache` it should read again.
4. **Secrets, metadata only.**
   - Run `agent-cli kv secret list --fields vault,name,expires,updated`.
     Every vault should be there, in the order `[azure] vaults` gives them.
   - Try `--expires-within 30d`, `--expired` and `--disabled` against what
     you know.
   - A vault with more than 25 secrets exercises `nextLink` paging.
5. **A value, safely.**
   - Run `agent-cli kv secret get NAME --fields value --output /tmp/v.txt`.
     stdout should show only the path, and the file should have mode 0600.
     Check that the value matches the portal byte for byte, including any
     trailing newline.
   - Then run `grep -rc 'the value' ~/.cache/agent-cli/`. Expect 0.
   - Then try a name that exists in two vaults. Expect exit 2 naming both.
6. **Refusals.**
   - Run a vault read that the login has no role on. Expect "no permission
     to read secrets".
   - Run a vault read from behind its firewall. Expect "blocked by the vault
     firewall".
   - Run a registry read without AcrPull. Expect "no permission (needs
     AcrPull…)". In each case the other vaults and registries should still
     list.
7. **Versions.** `agent-cli kv version list NAME` should list the portal's
   versions, newest first.
8. **Registries.**
   - Run `agent-cli acr repo list --fields registry,repository,tag_count,updated`.
     The counts and dates should be filled in, and a second run should
     come from the cache.
   - Run `agent-cli acr tag list REPO`. The newest tag should be first.
   - Run `agent-cli acr manifest get REPO TAG`.
     `docker pull <pull>` should accept the digest reference. Try it with a
     digest in place of the tag too.
9. **Throttling.** If a 429 ever shows, the command either waits once
   inside `--timeout` or exits 124 at once. It must never sleep past the
   deadline.

## AKS and k8s

1. **Connect.**
   - Run `agent-cli aks cluster connect CLUSTER --dry-run`. It should show
     both argv (`az aks get-credentials …` and `kubelogin convert-kubeconfig …`)
     and run neither.
   - Run it again without `--dry-run`. Expect `kubelogin` to report
     "converted", and `namespaces` to list the cluster's own namespaces.
   - Paste the `config` block into config.toml and trim the namespaces.
2. **Doctor.** Run `agent-cli doctor k8s`.
   - Expect the kubectl version, then one row per scope and namespace, each
     ok and under a second or two.
   - A `Forbidden` here is RBAC, not the tool.
   - A device-code prompt shows up as a timeout. It means the kubeconfig
     was not converted.
3. **Contexts.** `agent-cli k8s context list` should show `known: true` for
   every scope.
4. **Pods.**
   - Compare `agent-cli k8s pod list --cluster C --namespace N` with
     `kubectl get pods -n N` by eye. The names, STATUS words, READY and
     RESTARTS should be the same, and owners should show `Deployment/…`,
     not `ReplicaSet/…`.
5. **A crash loop, if there is one.**
   - `agent-cli k8s pod get POD` should show `last_termination`.
   - `agent-cli k8s event list --pod POD` should show BackOff events,
     newest first.
   - `agent-cli k8s pod logs POD --previous --tail 50` should show the
     crashed run.
6. **Big text.** Run `agent-cli k8s pod get POD --yaml` and `pod logs` on a
   chatty pod. Both should stay under the 12 KB guard, with the logs cut
   from the front.
7. **ConfigMaps and secrets.**
   - `agent-cli k8s configmap get NAME` should show the values, with binary
     keys shown as sizes.
   - `agent-cli k8s secret list` should show the key names and decoded sizes
     only.
   - `agent-cli k8s secret get NAME KEY --fields value --output /tmp/k.txt`
     should save a value that matches `kubectl get secret … | base64 -d`.
8. **Changes. Use a non-production namespace only.**
   - Delete one pod of a deployment with `pod delete POD --yes`. A new pod
     should appear, and the command should return without waiting.
   - Run `deployment restart NAME --yes`. The deployment should roll.
   - Run `deployment scale NAME --replicas N --yes`. `previous` should be
     the old count. Scale it back afterwards.
9. **Wrong on purpose.**
   - A scope whose context does not exist should exit 3 with "does not
     exist".
   - An unplugged network should give exit 1 with "Unable to connect", well
     inside `--timeout`.
   - `AGENT_CLI_READ_ONLY=1` should refuse every change and both
     `secret get` commands.

## Open questions only a live run can settle

- **Key Vault `api-version=7.4`.** No retirement has been announced. If a
  vault refuses it, change `API_VERSION` in `crates/azure/src/kv.rs`.
- **The registry exchange without `tenant`.** `az acr` sends the tenant;
  this code does not. If an exchange is refused, add it (see the `ponytail:`
  note in `crates/azure/src/acr.rs`).
- **`_manifests/{tag}`.** Is a tag accepted as the reference, or only a
  digest?
- **ABAC registries.** On these, `AcrPull` may not list the catalog. If
  `repo list` is empty but `tag list` works, the login needs Container
  Registry Repository Catalog Lister.
- **The Resource Graph projection.** Do `properties.currentKubernetesVersion`
  and `properties.powerState.code` come back as the code expects?
