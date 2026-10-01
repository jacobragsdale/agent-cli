# First live run

Every domain was built and tested without its service: fixtures stand in for
Azure DevOps, Resource Graph, Key Vault, the registries, Airflow and
Datadog, and `scripts/fake/kubectl` for kubectl. The TUI the Azure code came
from never had a live run either. This list is that run, one section per
domain. Work through a section in order on a machine that reaches the
service. The sql domain is left out: its tests already run against real
databases (`scripts/db-up.sh`).

**Before you paste anything into an issue or a commit, scrub it.**
Organization, project, vault, registry, cluster, subscription, tenant and
server names never go into this public repository. Replace them with `contoso`-style placeholders.

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
     the number: it decides the disk token cache (the `ponytail:` note in
     `crates/core/src/az.rs`).
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

### Open questions (Azure)

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

### Open questions (k8s)

- **One call for two kinds.** Does `kubectl get deployments,pods -o json`
  return both in one list, as `deployment list` expects?
- **"Rolled out".** Is the Progressing condition's `lastUpdateTime` the
  time the rollout finished?
- **Digests.** Does `imageID` carry the `sha256:` digest on this runtime?
- **SecretProviderClass.** Does `pod get` read `objects` YAML with quoted
  names and aliases into the right kv ids?
- **A rollout that gives up.** What exit code and message does `kubectl
  rollout status` give for `ProgressDeadlineExceeded` (`deployment wait`
  matches "exceeded its progress deadline") and for its own `--timeout`
  ("timed out waiting")? Does the watch outlive `--request-timeout=10s`,
  reconnecting until `--timeout`?

## Azure DevOps

Use a personal organization, never an employer's, and read only: set
`AGENT_CLI_READ_ONLY=1` for the whole section.

1. **Config.** Put `[ado]` with `org` and `project` in config.toml, then run
   `agent-cli doctor ado`. A 203 sign-in page should be exit 3, not a parse
   error.
2. **Every read once.** Run each read command that `agent-cli ado` lists
   with `--limit 5` where it takes one, then its `get` on an id it printed.
   Compare with the web UI by eye. Any write should be refused before it is
   sent.
3. **Shapes to confirm.** Each was built from the docs and recorded by hand:
   - **Work items:** WIQL `$top`; `timePrecision=true` with RFC 3339
     literals, including `[System.CreatedDate]`; `[Microsoft.VSTS.Common.Priority]
     IN (1, 2)`; comments with `order=desc`; identity search for `@me` and
     names; what a failed `rev` test returns.
   - **Builds:** `minTime`/`maxTime` with `queueTimeDescending`;
     `requestedFor` by display name (as `az pipelines runs list
     --requested-for` sends it) and `reasonFilter`; `triggerInfo["pr.number"]`;
     the log's `startLine` base; the pipelines run shape; approvals' `state`
     and `top`.
   - **Timeline issues:** `data.sourcepath`, `data.linenumber`,
     `data.columnnumber`, `data.code` and `data.logFileLineNumber`, and which
     tasks fill them (`DotNetCoreCLI`, `VSBuild`, `npm` with a problem
     matcher); a pull request build's `triggerInfo["pr.sourceSha"]`, the
     commit `run get` puts in `at`; a failed build policy's `status:
     rejected` with `context.buildId`.
   - **Test results** (`ado test list`): `test/runs?buildUri=` without date
     bounds; results carrying `errorMessage`, `stackTrace` and
     `failingSince.build.id` without `detailsToInclude`; test runs'
     `totalTests`, `passedTests` and `notApplicableTests`, which decide
     whether `run get` names `test list`; the
     `vstmr.dev.azure.com` host, if `dev.azure.com` redirects there.
   - **Pipeline preview:** api-version `7.1-preview.1`, its 400s and
     `PATH (Line: N, Col: M)`. A template from another repository
     (`x.yml@alias`) gets no hint.
   - **Pull requests:** search with `searchCriteria.minTime`/`maxTime`;
     `reviewerId` rows carrying the reviewer's own vote (a group member who
     has not voted is absent, which reads as none); auto-complete off as the
     empty GUID; `pullrequestquery` with `lastMergeCommit` for tag builds and
     `type: commit` for a commit inside a PR.
   - **Threads:** `?$iteration=N` placing `threadContext` at that iteration;
     `pr comment --at` landing on the latest iteration with offset 1 and no
     `pullRequestThreadContext`.
   - **Files and commits:** items `commitId` on the default branch, binary
     content with `includeContent`, short SHAs in `versionDescriptor`, an
     unknown branch as a 404 (TF401175) before the tag fallback;
     `recursionLevel=Full` size and paging on the largest repository;
     a missing path as a 404 (TF401174), which `file get` answers with a
     suffix match; commits `searchCriteria.itemPath` with `itemVersion` at a
     commit, and whether list items carry `parents` (`ado commit list`'s
     `diff` is null without them: read each commit, or diff from the first
     parent); `pullrequestquery` with several `lastMergeCommit` items.
   - **Diffs:** `diffs/commits` with `$top=1000`, paging,
     `allChangesIncluded` and the rename fields.
   - **Code Search:** `includeSnippet` filling `matches.content[].line` and
     `codeSnippet`; a repository filter needing a project; the
     `*.visualstudio.com` host; an organization without the extension.

## Airflow

Use an instance with `read_only = true` until the reads pass.

1. **Doctor.** Run `agent-cli doctor airflow`. Expect the metadatabase,
   scheduler, triggerer and dag-processor rows.
2. **Remote logging.** Run `agent-cli airflow task logs ID` on a task that
   failed yesterday. Either Airflow serves the log (remote logging is on) or
   the note says it has none and names the pod, which is gone by now. Record
   which: it decides what the pod hints are worth.
3. **Sign-in cost.** Time a call with password sign-in (one more POST per
   call) and with `token_cmd` (gcloud and aws are Python, 0.5 to 1 s). Above
   about 300 ms, add an expiry-aware 0600 token cache; a JWT's `exp` is
   readable.
4. **Shapes to confirm:**
   - `dagSources/{dag_id}` as JSON. `version_number` is not sent, so
     `source get` shows the latest version, not the one a failed run used.
   - `Filling up the DagBag from <path>` in task logs, and frame paths ending
     in `relative_fileloc` under versioned bundles.
   - XComs with `deserialize=true` (Airflow 2 needed
     `enable_xcom_deserialize_support`) and `stringify=false`.
   - Pools' `slots: -1` as unlimited.
   - `variable_key_pattern` and `connection_id_pattern` as `%`/`_` patterns.
   - Connections masking `password`, and `schema` holding an mssql
     database, which `sql_conn` relies on.

## Datadog

No org is reachable from the build machine, so this needs a sandbox: a
14-day trial org and a kind cluster running the Datadog Helm chart, a demo
app with the `tags.datadoghq.com` labels, and one monitor to mute and
unmute. Everything in it is synthetic, so answers captured there can be
committed once scrubbed.

1. **Doctor.** Run `agent-cli doctor dd`. A 403 should name the missing
   scope.
2. **Cluster names.** Does `kube_cluster_name` equal the k8s scope names?
   If not, k8s needs a Datadog alias for each scope, or `[[k8s.scope]] name`
   should default to the AKS cluster name.
3. **`service get`.** It reads sampled indexed spans: one fast, approximate
   call. Compare it with the exact trace metrics (`trace.<op>.hits` and
   `.errors`, two or three calls, since they need the operation name).
4. **Deploy events.** Does Datadog already ingest Kubernetes events and ADO
   deploy events? If so, "did anything deploy before this alert" needs no
   k8s or ado hop.
5. **Code links.** Do logs carry `git.commit.sha` and `git.repository_url`,
   or only spans? (The Agent may tag a container's telemetry from its
   image's `org.opencontainers.image.revision` and `.source` labels.) What
   is the attribute path of `error.stack` in a span search result? `at`
   reads logs' `attributes.attributes.error.stack` and spans'
   `attributes.custom.error.stack`, and the git tags from `attributes.tags`
   only; spans may carry them under `custom._dd.git.*` instead.
