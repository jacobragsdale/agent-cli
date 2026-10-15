# Plan: k8s and Azure coverage, and no read-only mode

This plan makes two changes. First, read-only mode goes. Second, k8s grows
from 14 commands to about 55, and kv, acr, aks and aisearch grow from 26 to
about 70 with two new domains (165 commands to about 250 in all). Today an agent drops to raw `kubectl` or `az`
for anything beyond pods, deployments, secrets and images: services and their
endpoints, network policies, PVCs and disks, nodes, identities, NSGs, routes
and private DNS. After this, it shouldn't have to.

## Decisions

1. **Coverage comes from a generic floor plus curated joins.** One command
   per kubectl kind and per `az` group would never finish: `az` alone has
   thousands.
   - `k8s object list|get KIND` reads any kind the API server serves, CRDs
     included: Gateway API, cert-manager, KEDA, SecretProviderClass,
     VolumeSnapshot, Cilium policies. `k8s kind list` shows what's there.
   - `arm resource list|get` and `arm query run` read any ARM resource:
     Resource Graph for lists, ARM GET for one, with the provider's newest
     stable api-version cached.
   - The floor still adds what a raw call lacks: scopes and allowlists, the
     id convention, `managedFields` and `last-applied` stripped, `--limit`,
     and errors that name the next step. So it keeps design.md's rule, "no
     one-to-one wrapper without a bound or a join", and design.md's "What
     was left out" gains a sentence saying so.
   - A curated command exists only where a join or a bound beats the raw
     object. Examples: a service with its ready endpoints and their pods; a
     PVC with its PV, storage class, attachment node and Azure disk; a
     subnet with its free IPs, NSG, routes and private endpoints. Everything
     else goes through the floor (see the last table).
   - Rejected: the spec-generated ARM runner that design.md sketches.
     `arm resource get` covers the same reads with no generator. Revisit it
     when writes on long-tail ARM types become a recurring task.

2. **Read-only goes entirely.** That means `AGENT_CLI_READ_ONLY` and the
   `read_only = true` keys on sql connections, airflow instances and
   controlm instances.
   - What still guards: `--dry-run`; `--yes` for anything destructive;
     `--reveal` or `--output` for secrets; and the credential itself (a
     Reader role, a read-only DB login, an Airflow Viewer). The setup guide
     recommends those credentials where it used to recommend read-only mode.
   - The config sections keep `deny_unknown_fields`, so a config that still
     has `read_only` fails to load (exit 3, naming the key). This is
     deliberate: whoever wrote that line believes it protects them.

3. **Domains.**
   - `aks`, `kv` and `acr` grow.
   - Two new domains: `arm` (the floor, identity, activity, changes,
     policy, metrics) and `net` (vnets, subnets, NSGs, routes, private
     endpoints, DNS, public IPs, load balancers, application gateways).
     Both live in the azure crate and sign in through `[azure]` and its
     `subscriptions` allowlist, so there's no new config section.
   - `aks` passes five commands, so TODO.md's "fold aks into k8s" is
     dropped.
   - The k8s and azure crates stay independent: a join is an id one side
     prints and the other takes, never a call across. k8s prints what the
     cluster already holds (a disk's ARM id from `volumeHandle`, a VM's
     from `providerID`, a workload identity's client id); the Azure side
     names the k8s object where it can (a load balancer frontend's
     service).
   - Azure Monitor, Blob storage and Entra are out of scope.

4. **Every curated k8s `get` includes its object's recent events**: the
   five newest, matched by `involvedObject.uid`, as `kubectl describe` shows
   them. That's the call an agent would make next. `event list` also gains
   `--object KIND/NAME` for other objects.

5. **Cluster-scoped kinds take `cluster/name` ids.** These are node, pv,
   storageclass and namespace. `At` gains `cluster_wide(ctx)` and
   `named_cluster(ctx, raw)`, so a two-part ref is never read as
   `namespace/name`.

6. **Writes are added only for a recurring task** (phase 6, plus a few in
   phases 7 and 10). `Target::write` takes its `Effect`: `cronjob run` is a
   Write, not Destructive.

## Phase 0: groundwork (one commit each)

**0a. Take out read-only.**
- Core:
  - `ctx.rs`: the field, `read_only_on`, the refusal in `write`,
    `READ_ONLY_HINT`.
  - `dispatch.rs`: the precheck, the overview argument, the doctor row.
  - `discover.rs`: the `Config:` guard.
  - `config.rs`: the variable in `config`'s output.
  - `testing.rs`: `Setup::read_only`, `assert_read_only_refuses`.
  - Also the `lib.rs` doc, `tests/runtime.rs`, `tests/common/mod.rs` and
    the core card.
- Every domain: delete the `read_only_mode_refuses_every_change…` test in
  ado, dd, confluence, k8s, airflow and controlm, and in
  `crates/cli/src/main.rs` (and its line in the cli card). `assert_dry_run`
  stays, and it still proves that every write goes through `ctx.write`.
- Per-instance keys:
  - sql: `config.rs`, the refusal in `query/mod.rs`, the `read_only` field
    in `connection list`, `testing.rs`, and the tests in `query/run.rs`.
  - airflow: `client.rs`, the `instance list` field, the test in
    `dag/update.rs`, and `live_airflow.rs`.
  - controlm: `client.rs` and the test in `job/run.rs`.
  - The three crate cards.
- The world: the two `read_only = true` lines in
  `fixtures/world/config.toml`, the refusal check in `world_cross.rs`,
  `facts/airflow.md`, `tests/common/world.rs` and `scripts/trial-env.sh`.
- Docs:
  - README and the root AGENTS.md (three places).
  - design.md (One door, Where the code came from, Past hand-written
    commands).
  - add-a-command.md and add-a-domain.md.
  - set-up-your-services.md: replace "read-only for the first week" with
    least-privilege credentials.
  - first-live-run.md and config.example.toml.
  - The comment in `scripts/check.sh` and the gate in `live_ado.rs`.
  - The comments in the ado, dd, confluence and airflow clients.
  - Regenerate the reference (the sql and airflow pages change).
- Check: `grep -rn 'READ_ONLY\|read_only\|read-only'` over crates, scripts,
  docs, README.md, AGENTS.md, config.example.toml and
  `fixtures/world/config.toml` should find only database read-only logins,
  `PUP_READ_ONLY` and C#'s `readonly`.

**0b. The fake kubectl reads its objects from a file** (from TODO.md). The
built-in contexts move to `crates/k8s/fixtures/cluster.json`, in the same
shape as `fixtures/world/kubectl.json`. `get KIND` (including comma-joined
kinds) then answers for any kind the file holds, with `-l` and
`--field-selector involvedObject.uid=`. A new kind becomes data instead of
Python.

**0c. Shared k8s helpers**, in a new `crates/k8s/src/shape.rs`, because
`kubectl.rs` is already at the 600-line limit:
- `At::cluster_wide` and `At::named_cluster`.
- `Target::write(effect, …)`.
- `events_for(uid)`.
- `trim(manifest)`: drops `managedFields` and `last-applied`, keeps
  `status`.
- `finished`'s not-found hint, generalised from four kinds to every curated
  one, falling back to `k8s object list KIND`.

A joined `get` makes one `kubectl get a,b,c -o json` call where it can, not
one process per kind.

## Phase 1: the floor

| Command | Effect | Behind it | Returns |
|---|---|---|---|
| `k8s kind list` | Read | `kubectl api-resources -o wide` | name, short names, group/version, namespaced, verbs |
| `k8s object list KIND [NAME]` | Read | `get KIND -o json`, `--label` | id, kind, namespace, age, ready (from a `Ready` or `Available` condition), status |
| `k8s object get KIND REF` | Read | `get KIND NAME -o json`; whether KIND is namespaced comes from `api-resources` (cached) | the trimmed manifest, or `--yaml` |
| `arm subscription list` | Read | ARM `/subscriptions` | id, name, state, tenant |
| `arm group list` | Read | Resource Graph `resourcecontainers` | id, name, subscription, location, tags |
| `arm resource list [NAME]` | Read | Resource Graph `resources`: `--type`, `--group`, `--tag`, `--location`, `--subscription` | id, name, type, group, location, sku, provisioning state |
| `arm resource get REF` | Read | ARM `GET {id}`; REF is an ARM id (a child collection path too), a portal URL, or a name with `--type` | the resource |
| `arm query run KQL` | Read | Resource Graph through `Request::query`; `-` or `--kql-file` | the rows, paged to `--limit` |

## Phase 2: k8s network

| Command | Effect | Behind it | Returns |
|---|---|---|---|
| `k8s service list [NAME]` | Read | services and endpoint slices | id, type, cluster IP, external IP or host, ports (`80→8080/TCP`), selector, ready/total endpoints |
| `k8s service get REF` | Read | service, its endpoint slices, their pods | ports with named target ports resolved; endpoints `[{pod, ip, ready, node, zone}]`; why there are none (no selector, no match, none ready); Azure LB annotations; events |
| `k8s ingress list` | Read | ingresses | id, class, hosts, TLS secrets, address |
| `k8s ingress get REF` | Read | ingress, each backend's service and endpoints | rules `[{host, path, service, port, ready}]`, TLS `[{hosts, secret, exists}]`, default backend, class and controller, events |
| `k8s networkpolicy list` | Read | network policies; `--pod REF` keeps those that select it | id, pod selector, policy types, rule counts, `default_deny` |
| `k8s networkpolicy get REF` | Read | one policy and the pods it selects | rules, flattened: direction, peers (pods, namespaces, CIDRs), ports |
| `k8s context get [CLUSTER]` | Read | `version`, `--raw /readyz?verbose`, kube-system's `kube-dns` and `coredns-custom` | server version, failing health checks, DNS service IP, CNI (from kube-system's daemonsets), node count |

`pod get` gains `ips`, `host_network`, `dns_policy`, `ports`, `probes`,
`resources`, `service_account` (an id), `volumes` (a PVC id, configmap,
secret or CSI), `node_selector` and `tolerations`. `pod list` gains `--node`
and `--status`.

## Phase 3: k8s storage

| Command | Effect | Behind it | Returns |
|---|---|---|---|
| `k8s pvc list [NAME]` | Read | PVCs and the pods that mount them | id, status, capacity, requested, access modes, storage class, volume, `used_by` |
| `k8s pvc get REF` | Read | PVC, its PV, its storage class, `volumeattachments`, pods | status and conditions (resize pending); `pv {id, reclaim, driver, disk or share (ARM id from volumeHandle), zone}`; storage class `{provisioner, sku, binding mode, expansion}`; `attached_to` node; `used_by`; events (Multi-Attach, ProvisioningFailed) |
| `k8s pv list` | Read | PVs | id, status, claim (a PVC id), capacity, reclaim, storage class, driver, disk |
| `k8s pv get REF` | Read | PV and its attachment | as `pvc get`'s `pv`, plus node affinity and attachment |
| `k8s storageclass list` | Read | storage classes | name, default, provisioner, parameters, binding mode, expansion, reclaim |

VolumeSnapshot, CSIDriver and CSINode are reached through the floor.

## Phase 4: k8s workloads

| Command | Effect | Returns |
|---|---|---|
| `k8s deployment get REF` | Read | strategy, conditions, revisions `[{revision, images, change_cause, created, replicas}]` (from its replica sets), containers `{image, resources, probes, env names and the secrets and configmaps they read, never values}`, volumes, events |
| `k8s statefulset list`, `get` | Read | ready/replicas, current vs update revision, partition, claim templates and their PVC ids, events |
| `k8s daemonset list`, `get` | Read | desired, ready, updated, unavailable, node selector, events |
| `k8s job list`, `get` | Read | status, completions, failures, duration, owner cronjob, pods, events |
| `k8s cronjob list`, `get` | Read | schedule, time zone, suspended, last schedule and success, active jobs, recent jobs |
| `k8s hpa list` | Read | target, min/max/current/desired, each metric's current vs target, `ScalingLimited` and `AbleToScale` reasons |
| `k8s pdb list` | Read | selector, min available or max unavailable, allowed disruptions (0 blocks drains and upgrades) |

This resolves `owner_of`'s ponytail note: a Job's CronJob.

## Phase 5: k8s nodes, namespaces, identity

| Command | Effect | Behind it | Returns |
|---|---|---|---|
| `k8s node list` | Read | nodes, all pods' requests, `top nodes` when metrics-server answers | name, ready, schedulable, pool, zone, version, taints, CPU and memory requested and used against allocatable, pressure conditions |
| `k8s node get REF` | Read | node and its pods | conditions, addresses, pod CIDR, capacity, allocatable, pods with requests, `vm` (the ARM id from providerID), `nodepool` (an aks id), events |
| `k8s namespace list` | Read | namespaces and quotas | name, status, labels, quota used/hard, in scope |
| `k8s quota list` | Read | resource quotas and limit ranges | namespace, resource, used, hard, defaults |
| `k8s serviceaccount list`, `get` | Read | service accounts, the pods using them | `client_id` (workload identity), which `arm identity get` takes; pull secrets, automount, pods; `get` adds what it may do |
| `k8s access list` | Read | `auth can-i --list`, `--as` a serviceaccount id or `@me` | verb, resources, names |

`pod list` and `node list` show usage from `kubectl top` when metrics-server
answers. When it doesn't, they print a note, never an error.

## Phase 6: k8s writes

| Command | Effect | Behind it |
|---|---|---|
| `k8s cronjob run REF` | Write | `create job --from=cronjob/NAME`; next: `k8s job get` |
| `k8s job delete REF` | Destructive | `delete job` |
| `k8s statefulset restart`, `scale` | Destructive | `rollout restart`, `scale` |
| `k8s daemonset restart` | Destructive | `rollout restart` |
| `k8s deployment update REF --revision N` | Destructive | `rollout undo --to-revision` |
| `k8s node update REF --schedulable true\|false` | Destructive | `cordon`, `uncordon` |
| `k8s pod run REF -- ARGV…` | Destructive | `exec` under the deadline, output bounded by `--tail`: `nslookup`, `env`, `nc -zv` from where the app runs |

Left out:
- `drain`: AKS upgrades drain on their own, and it would need a new verb
  for a rare task.
- `apply`: changes go through Git.
- `port-forward`: it runs indefinitely.

## Phase 7: aks

| Command | Effect | Behind it | Returns |
|---|---|---|---|
| `aks cluster get REF` | Read | managedClusters GET | version, tier, power and provisioning state; network profile (plugin, mode, policy, pod and service CIDRs, DNS IP, outbound type and IPs); API server (private, FQDN, authorized ranges); OIDC issuer and workload identity; kubelet `identity` (ARM id); addons; auto-upgrade channel; maintenance windows; node resource group; `k8s_scope` |
| `aks nodepool list CLUSTER` | Read | agentPools | id (`cluster/pool`), mode, VM size, count, autoscale min/max, version, node image, OS SKU, zones, `subnet` (ARM id), max pods, taints, labels, state |
| `aks nodepool get REF` | Read | agentPool and its upgrade profile | as the list, plus surge settings and the newest node image available |
| `aks upgrade list CLUSTER` | Read | `upgradeProfiles/default` and the pools' profiles | control-plane and pool versions available, node image upgrades |
| `aks nodepool scale REF --count N` | Destructive | agentPools PUT | |
| `aks command run CLUSTER -- kubectl …` | Destructive | `runCommand`, then poll `commandResults` (timeout 100) | for a private API server that WSL can't reach |

## Phase 8: net (new domain)

Ids are ARM ids. A name with `--group` (and `--subscription`) or a portal URL
also works.

| Command | Effect | Behind it | Returns |
|---|---|---|---|
| `net vnet list` | Read | Resource Graph | id, address space, subnets, peerings and their state, DNS servers |
| `net vnet get REF` | Read | vnet GET and `usages` | subnets `[{name, prefix, used/limit IPs, nsg, route table, nat gateway, delegation}]`, peerings `[{remote, state, forwarded and gateway transit}]`, DNS servers |
| `net subnet get REF` | Read | subnet GET, usages, Resource Graph for IP holders | prefix, used/limit IPs, NSG, route table, NAT gateway, service endpoints, delegations, private endpoints, IP holders (AKS pool, private endpoint, app gateway) |
| `net nsg list` | Read | Resource Graph | id, rule count, subnets, NICs |
| `net nsg get REF` | Read | NSG GET | rules in both directions by priority, defaults included; what it's attached to |
| `net nic get REF` | Read | NIC GET, `effectiveNetworkSecurityGroups`, `effectiveRouteTable` (long-running POSTs) | IPs, subnet, NSG, effective rules and routes |
| `net route list` | Read | route tables | table, route, prefix, next hop, subnets, BGP propagation |
| `net endpoint list`, `get` | Read | private endpoints | target (ARM id, group), connection state, IPs, DNS zone group, FQDN and its record |
| `net zone list` | Read | public and private DNS zones | name, private, record count, vnet links `[{vnet, registration}]` |
| `net record list ZONE` | Read | record sets | name, type, TTL, values |
| `net ip list` | Read | public IPs | address, SKU, allocation, held by (LB frontend, NAT gateway, app gateway) |
| `net lb list`, `get` | Read | load balancers | frontends (IP, and the k8s service that owns it, from the frontend's name), rules, probes, backend pools |
| `net appgw list`, `get` | Read | application gateways; `--health` adds `backendhealth` (POST, timeout 100) | listeners, rules, backend pools and settings, probes, WAF mode, backend health |

## Phase 9: arm, beyond the floor

| Command | Effect | Behind it | Returns |
|---|---|---|---|
| `arm activity list` | Read | the activity log: `--resource`, `--group`, `--caller` (`@me`), `--status`, `--since`, `--until` | when, caller, operation, status, resource, error |
| `arm change list` | Read | Resource Graph `resourcechanges` | when, resource, change type, properties `[{path, before, after}]` |
| `arm role list` | Read | role assignments: `--principal`, `--scope`, with role names resolved | principal id and type, role, scope |
| `arm identity list`, `get` | Read | user-assigned identities and their federated credentials | client and principal ids (REF takes a client id too); federated credentials `[{issuer, subject}]` with the subject as a k8s serviceaccount id; roles |
| `arm policy list` | Read | Policy Insights `policyStates/latest`, non-compliant only | policy, effect, resource, reason (for `RequestDisallowedByPolicy`) |
| `arm metric list REF` | Read | `microsoft.insights/metrics`: `--metric`, `--since`, `--interval` | metric, unit, bounded points (disk IOPS consumed %, LB probe health …) |

## Phase 10: kv and acr

| Command | Effect | Behind it | Returns |
|---|---|---|---|
| `kv vault get REF` | Read | vault GET | RBAC or access policies, soft delete and purge protection, network ACLs, private endpoints, public access |
| `kv key list`, `get` | Read | data plane `/keys` | type, size or curve, operations, enabled, expiry, rotation policy; never key material |
| `kv certificate list`, `get` | Read | data plane `/certificates`; `--expires-within` | subject, SANs, issuer, thumbprint, expiry, autorenew; `get` adds the policy and any pending operation |
| `kv secret list --deleted` | Read | `/deletedsecrets` | deleted, scheduled purge |
| `kv secret update REF -` | Write | PUT; the value comes from stdin or `--value-file` into a `Secret` | the new version's id, never the value |
| `acr registry get REF` | Read | registry GET, replications, webhooks | SKU, admin user, public access, network rules, private endpoints, retention, replications, webhooks |
| `acr run list`, `get` | Read | ACR Tasks runs | id, task, status, trigger, images, duration, error |
| `acr run logs REF` | Read | `listLogSasUrl`, then GET the blob with no token; the SAS URL is a `Secret` and is never printed | the tail (`--tail`) |
| `acr tag delete REF` | Destructive | data plane DELETE | |

## Reached through the floor, not curated

| `az` group | How |
|---|---|
| firewall, NAT gateway, bastion, VPN, ExpressRoute, Front Door, CDN | `arm resource list --type …`, `arm resource get` |
| postgres, sql, mysql, cosmos, redis: firewall and vnet rules | `arm resource get SERVER_ID/firewallRules` (a child collection) |
| servicebus, eventhubs: queue and dead-letter counts | `arm resource get` (`countDetails`) |
| app service, functions, container apps: config | `arm resource get`. App settings are a `list` POST that returns secrets, so they're deliberately not covered |
| ARM deployments, locks, tags | `arm resource get …/deployments`, `arm resource list` |
| Defender findings for an image | `arm query run` over `securityresources` |
| the VMSS instance behind a node, a PV's disk | `arm resource get` on `k8s node get`'s `vm` and `k8s pvc get`'s `disk` |

## The world and trials

Each phase adds its kinds to `fixtures/world/kubectl.json` or its answers
to `http/azure.json` (scrubbed, contoso names), facts to
`facts/<domain>.md`, and checks to `world_<domain>.rs`. `world_cross.rs`
gains four traces, and each one is also a trial task:

1. **Private DNS.** `worker` times out reaching `kv-contoso`. `k8s pod run
   … nslookup` resolves the public IP. `net endpoint get` shows a private
   IP. `net zone list` shows `privatelink.vaultcore.azure.net` isn't linked
   to the AKS vnet.
2. **Multi-Attach.** `orders-db-0` is stuck in ContainerCreating after a
   node pool upgrade. `k8s pvc get` shows the disk still attached to the old
   node, and `k8s node list` shows that node NotReady.
3. **Pending pods.** Events say `0/3 nodes available: insufficient cpu`.
   `k8s node list` shows requests at 95%, `aks nodepool get` shows the
   autoscaler at its maximum, and `k8s hpa list` shows the HPA at max
   replicas.
4. **Workload identity 403.** `worker` gets a 403 from Key Vault. `k8s
   serviceaccount get` leads to `arm identity get`, where the federated
   subject matches. `arm role list --principal` shows no Key Vault role on
   the vault.

These four traces also make up TODO.md's raw-tools trial, run against
agent-cli and against `kubectl` with `az`. `arm` and `net` earn their place
as domains if agent-cli wins at least half the tasks. Every miss becomes a
fix, a test or a labeled query.

## Risks

- **Search.** Some resource names are already taken in other domains:
  `service` (dd, aisearch), `object` (sql), `query` (ado, sql), `metric`
  (dd), `activity` (ado), `run` (ado, airflow). Also, `object get` competes
  with every curated get. Each phase writes its queries first, and the gate
  (top-1 80%, top-5 95%) has to hold with about 85 more commands.
- **Overview.** There will be 13 domains. Check the 1 KB test in phase 1.
- **Latency.** A joined get is still one kubectl process (comma-joined
  kinds) plus the pods it needs. Measure it on a real AKS before adding
  parallelism.

## Open questions (settle at work; mark them `VERIFY(work)` in code)

1. Does Azure return effective NSG rules and routes for a VMSS instance NIC
   (an AKS node)? If not, `net nic get` combines the subnet's and the NIC's
   NSG rules and says that's what it did.
2. Is the work AKS's API server private? If so, `aks command run` moves to
   phase 1.
3. Which CNI and ingress does work use? Azure CNI overlay or Cilium (Cilium
   policies go through the floor), and AGIC, app routing or Gateway API.
   This decides whether `net appgw`, `k8s ingress` or Gateway objects come
   first.
4. Does `resourcechanges` cover the work subscriptions (14 days by
   default)?
5. Which api-version should `arm resource get` pick when a provider's newest
   is preview only?

## Done when

- Each phase passes `scripts/check.sh --all` and is pushed on its own.
- The k8s and azure cards describe what's new, and the root AGENTS.md
  layout line names `arm` and `net`.
- The four traces pass in `world_cross.rs`, and a trial against the raw
  tools is recorded.
- The k8s and Azure rows of TODO.md's live-run item have run against a real
  AKS at work.
- This plan's open items move to TODO.md, and the plan is deleted.
