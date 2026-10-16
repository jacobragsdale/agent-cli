# k8s — Kubernetes (55 commands)

| Command | Effect | Summary |
|---|---|---|
| [`k8s pod list`](#k8s-pod-list) | read | List pods: status, ready, restarts, owner; or those mounting a Key Vault secret |
| [`k8s pod get`](#k8s-pod-get) | read | Describe a pod: containers, images, states, last termination reason, owner |
| [`k8s pod logs`](#k8s-pod-logs) | read | Read the tail of a pod's log, or of its previous run |
| [`k8s pod run`](#k8s-pod-run) | destructive | Run a command in a pod's container, from where the app runs (kubectl exec) |
| [`k8s pod delete`](#k8s-pod-delete) | destructive | Delete a pod so its controller replaces it (a bare pod is gone for good) |
| [`k8s event list`](#k8s-event-list) | read | List Kubernetes events, newest first: warnings, back-offs, failed pulls |
| [`k8s service list`](#k8s-service-list) | read | List Kubernetes services: type, cluster IP, external IP, ports, ready endpoints |
| [`k8s service get`](#k8s-service-get) | read | Show a Kubernetes service: ports, endpoint pods and IPs, or what leaves it empty |
| [`k8s ingress list`](#k8s-ingress-list) | read | List ingresses: the hosts each serves, class, TLS secrets, address |
| [`k8s ingress get`](#k8s-ingress-get) | read | Show an ingress: each host and path's service and ready endpoints, TLS secrets |
| [`k8s networkpolicy list`](#k8s-networkpolicy-list) | read | List network policies, or those that apply to one pod: selector, default deny |
| [`k8s networkpolicy get`](#k8s-networkpolicy-get) | read | Show a network policy's rules in words: peers, ports, and the pods it selects |
| [`k8s pvc list`](#k8s-pvc-list) | read | List persistent volume claims: status, size, storage class, pods mounting each |
| [`k8s pvc get`](#k8s-pvc-get) | read | Show a volume claim: Azure disk or share, class, attachment, mounts, events |
| [`k8s pv list`](#k8s-pv-list) | read | List persistent volumes: claim, capacity, reclaim policy, Azure disk or share |
| [`k8s pv get`](#k8s-pv-get) | read | Show a persistent volume: Azure disk id, zone affinity, attachment, events |
| [`k8s storageclass list`](#k8s-storageclass-list) | read | List storage classes: default, provisioner, SKU, binding mode, volume expansion |
| [`k8s deployment list`](#k8s-deployment-list) | read | List deployments: ready pods, images with tag and digest, when they rolled out |
| [`k8s deployment get`](#k8s-deployment-get) | read | Describe a deployment: revision history, strategy, conditions, env, probes |
| [`k8s deployment restart`](#k8s-deployment-restart) | destructive | Restart a deployment, replacing its pods one at a time |
| [`k8s deployment scale`](#k8s-deployment-scale) | destructive | Scale a deployment to a number of replicas |
| [`k8s deployment update`](#k8s-deployment-update) | destructive | Roll a deployment back to an earlier version of its pod template |
| [`k8s deployment wait`](#k8s-deployment-wait) | read | Wait for a rollout: exit 0 once rolled out, 1 if it failed, 124 if still going |
| [`k8s statefulset list`](#k8s-statefulset-list) | read | List statefulsets: ready pods, images, volume claim templates |
| [`k8s statefulset get`](#k8s-statefulset-get) | read | Describe a statefulset: revisions, partition, each pod's volume claim, events |
| [`k8s statefulset restart`](#k8s-statefulset-restart) | destructive | Restart a statefulset, replacing its pods one at a time from the highest |
| [`k8s statefulset scale`](#k8s-statefulset-scale) | destructive | Scale a statefulset to a number of replicas |
| [`k8s daemonset list`](#k8s-daemonset-list) | read | List daemonsets, kube-system's too: desired, ready, unavailable per node |
| [`k8s daemonset get`](#k8s-daemonset-get) | read | Describe a daemonset, kube-system's too: unready pods, update strategy, events |
| [`k8s daemonset restart`](#k8s-daemonset-restart) | destructive | Restart a daemonset, replacing its pod on each node in turn |
| [`k8s job list`](#k8s-job-list) | read | List jobs: status, completions, failures, duration, the cronjob that made each |
| [`k8s job get`](#k8s-job-get) | read | Show why a job failed: its tries, conditions, the cronjob behind it, events |
| [`k8s job delete`](#k8s-job-delete) | destructive | Delete a job and its pods, stopping it if it still runs |
| [`k8s cronjob list`](#k8s-cronjob-list) | read | List cronjobs: schedule, time zone, suspended, last run and last success |
| [`k8s cronjob get`](#k8s-cronjob-get) | read | Describe a cronjob: recent runs with status, why one did not run |
| [`k8s cronjob run`](#k8s-cronjob-run) | write | Run a cronjob now, as a job made from its template, off its schedule |
| [`k8s hpa list`](#k8s-hpa-list) | read | List autoscalers: min, max, current and desired replicas, metrics vs target |
| [`k8s pdb list`](#k8s-pdb-list) | read | List disruption budgets (PDBs): allowed disruptions; 0 blocks drains, upgrades |
| [`k8s node list`](#k8s-node-list) | read | List nodes: ready, cordoned, taints, CPU and memory requests and usage |
| [`k8s node get`](#k8s-node-get) | read | Show a node: conditions, requests, its Azure VM and AKS pool, events |
| [`k8s node update`](#k8s-node-update) | destructive | Cordon or uncordon a node: whether new pods may be scheduled on it |
| [`k8s namespace list`](#k8s-namespace-list) | read | List namespaces: status, labels, quota used of hard, whether the scope reads it |
| [`k8s quota list`](#k8s-quota-list) | read | List resource quotas used of hard, and limit ranges' default requests and limits |
| [`k8s serviceaccount list`](#k8s-serviceaccount-list) | read | List service accounts: their Azure client id, the pods that run as each |
| [`k8s serviceaccount get`](#k8s-serviceaccount-get) | read | Show a service account: its Azure client id, pods, what RBAC lets it do |
| [`k8s access list`](#k8s-access-list) | read | List what RBAC allows you or a service account to do in a namespace (auth can-i) |
| [`k8s configmap list`](#k8s-configmap-list) | read | List configmaps and their keys |
| [`k8s configmap get`](#k8s-configmap-get) | read | Show a configmap's keys and values (binary keys show their size only) |
| [`k8s secret list`](#k8s-secret-list) | read | List Kubernetes secrets: type, key names and sizes (never values) |
| [`k8s secret get`](#k8s-secret-get) | reveal | Decode one key of a Kubernetes secret (needs --reveal, or --output FILE) |
| [`k8s context list`](#k8s-context-list) | read | List the configured cluster scopes and whether kubectl knows each context |
| [`k8s context get`](#k8s-context-get) | read | Check a cluster: server version, failing readiness checks, DNS address, CNI |
| [`k8s kind list`](#k8s-kind-list) | read | List every kind a cluster has, CRDs included: names, short names, API group |
| [`k8s object list`](#k8s-object-list) | read | List objects of any kind, custom resources (CRDs) too: id, age, ready, status |
| [`k8s object get`](#k8s-object-get) | read | Show one object of any kind, a custom resource (CRD) too: manifest and status |

### k8s pod list

```text
agent-cli k8s pod list — List pods: status, ready, restarts, owner; or those mounting a Key Vault secret
  <name> str                    Part of the pod name
  --cluster str                 The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str               Defaults to the scope's only namespace
  --label str[]                 Only pods with this label: app=api, or a key alone (repeatable, all must hold)
  --kv str                      Only pods mounting this Key Vault secret: VAULT/NAME, as `kv secret list` prints it
  --node str                    Only pods on this node, as `pod list` prints it
  --status running|pending|succeeded|failed|unknown|crashloopbackoff|imagepullbackoff|errimagepull|containercreating|completed|error|oomkilled|…  Only pods whose phase or status word is this
  --limit int                   (default 50)
Returns: [{id,name,namespace,status,ready,restarts,age,node,owner,cpu_usage,memory_usage}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s pod list --cluster qa --namespace dev --fields id,status,restarts,owner
```

### k8s pod get

```text
agent-cli k8s pod get — Describe a pod: containers, images, states, last termination reason, owner
 *<pod> str        The pod: its id (cluster/namespace/name), namespace/name, or name
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
  --yaml           The manifest as YAML text instead
Returns: {id,name,namespace,status,ready,restarts,age,node,ip,ips[],host_network,dns_policy,service_account,owner,containers[{name,image,digest,ready,restarts,state,last_termination}],ports[{container,name,port,protocol}],probes[{container,kind,action,initial_delay,period,failure_threshold}],requests,limits,volumes[{name,kind,source}],node_selector,tolerations[],secret_refs[{via,secret,keys[],class,kv[]}],conditions[{type,status,reason,message}],events[{type,reason,object,message,count,last_seen,namespace}],labels,text}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s pod get qa/dev/orders-api-7d9f5b-abc12 --fields status,containers,secret_refs
```

### k8s pod logs

```text
agent-cli k8s pod logs — Read the tail of a pod's log, or of its previous run
 *<pod> str        The pod: its id (cluster/namespace/name), namespace/name, or name
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
  --container str  Which container; kubectl picks the default one when left out
  --tail int       How many of the last lines (default 200)
  --previous       The run before the last restart: where a crash loop says why
  --since time     Only lines logged after this
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: {pod,namespace,container,lines,text}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s pod logs qa/dev/orders-worker-5c4d3e-q8zt --previous --tail 50
```

### k8s pod run

```text
agent-cli k8s pod run — Run a command in a pod's container, from where the app runs (kubectl exec)
 *<pod> str        The pod: its id (cluster/namespace/name), namespace/name, or name
 *<argv> str[]     The command after `--`: nslookup HOST, env, cat /etc/resolv.conf, nc -zv HOST PORT
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
  --container str  Which container; defaults to the pod's only one
  --tail int       How many of the last lines of stdout, and of stderr, to keep (default 200)
Returns: {id,container,command,exit_code,stdout,stderr}
Destructive: needs --yes; --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s pod run prod/web/api-7d9f8c6b5-x2k4q --yes -- nslookup kv-contoso-prod.vault.azure.net
```

### k8s pod delete

```text
agent-cli k8s pod delete — Delete a pod so its controller replaces it (a bare pod is gone for good)
 *<pod> str        The pod: its id (cluster/namespace/name), namespace/name, or name
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
Returns: {cluster,namespace,object,said,replicas,previous}
Destructive: needs --yes; --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s pod delete orders-api-7d9f5b-abc12 --cluster qa --namespace dev
```

### k8s event list

```text
agent-cli k8s event list — List Kubernetes events, newest first: warnings, back-offs, failed pulls
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
  --pod str        Only events about this pod: its id, namespace/name or name
  --object str     Only events about this object, as KIND/NAME (pvc/data-orders-db-0, node/aks-user-27182818-vmss000001); NAME may be its id
  --since time     Only events last seen after this
  --limit int      (default 50)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: [{type,reason,object,message,count,last_seen,namespace}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s event list --pod qa/dev/orders-worker-5c4d3e-q8zt --since 1h --fields type,reason,message,last_seen
```

### k8s service list

```text
agent-cli k8s service list — List Kubernetes services: type, cluster IP, external IP, ports, ready endpoints
  <name> str       Part of the service name
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
  --limit int      (default 50)
Returns: [{id,name,namespace,type,cluster_ip,external[],ports[],selector,ready,age}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s service list --cluster qa --namespace dev --fields id,type,ports,ready
```

### k8s service get

```text
agent-cli k8s service get — Show a Kubernetes service: ports, endpoint pods and IPs, or what leaves it empty
 *<service> str    The service: its id (cluster/namespace/name), namespace/name, or name
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
Returns: {id,name,namespace,type,cluster_ip,external[],ports[{name,port,target_port,target_name,protocol,node_port}],selector,ready,endpoints[{pod,ip,ready,node,zone}],why_none,azure,events[{type,reason,object,message,count,last_seen,namespace}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s service get qa/dev/orders-api --fields ports,ready,endpoints,why_none
```

### k8s ingress list

```text
agent-cli k8s ingress list — List ingresses: the hosts each serves, class, TLS secrets, address
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
  --limit int      (default 50)
Returns: [{id,name,namespace,class,hosts[],tls[],address[],age}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s ingress list --cluster qa --namespace dev --fields id,hosts,address
```

### k8s ingress get

```text
agent-cli k8s ingress get — Show an ingress: each host and path's service and ready endpoints, TLS secrets
 *<ingress> str    The ingress: its id (cluster/namespace/name), namespace/name, or name
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
Returns: {id,name,namespace,class,controller,address[],rules[{host,path,service,port,ready}],default_backend{host,path,service,port,ready},tls[{hosts[],secret,exists}],events[{type,reason,object,message,count,last_seen,namespace}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s ingress get qa/dev/orders --fields rules,tls
```

### k8s networkpolicy list

```text
agent-cli k8s networkpolicy list — List network policies, or those that apply to one pod: selector, default deny
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
  --pod str        Only the policies that select this pod: its id, namespace/name or name
  --limit int      (default 50)
Returns: [{id,name,namespace,pod_selector,types[],ingress_rules,egress_rules,default_deny}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s networkpolicy list --pod qa/dev/orders-api-7d9f5b-abc12 --fields id,types,default_deny
```

### k8s networkpolicy get

```text
agent-cli k8s networkpolicy get — Show a network policy's rules in words: peers, ports, and the pods it selects
 *<policy> str     The policy: its id (cluster/namespace/name), namespace/name, or name
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
Returns: {id,name,namespace,pod_selector,pods[],types[],default_deny,rules[{direction,peers[],ports[]}],events[{type,reason,object,message,count,last_seen,namespace}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s networkpolicy get qa/dev/allow-orders-api --fields pods,rules
```

### k8s pvc list

```text
agent-cli k8s pvc list — List persistent volume claims: status, size, storage class, pods mounting each
  <name> str       Part of the claim name
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
  --limit int      (default 50)
Returns: [{id,name,namespace,status,capacity,requested,access_modes[],storage_class,volume,used_by[],age}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s pvc list --cluster qa --namespace dev --fields id,status,capacity,used_by
```

### k8s pvc get

```text
agent-cli k8s pvc get — Show a volume claim: Azure disk or share, class, attachment, mounts, events
 *<pvc> str        The claim: its id (cluster/namespace/name), namespace/name, or name
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
Returns: {id,name,namespace,status,capacity,requested,access_modes[],conditions[{type,status,reason,message}],pv{id,status,claim,capacity,access_modes[],reclaim,storage_class,driver,disk,share{resource_group,account,name},zone,age},storage_class,class{name,default,provisioner,parameters,binding_mode,expansion,reclaim},attached_to,used_by[],events[{type,reason,object,message,count,last_seen,namespace}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s pvc get qa/dev/orders-data --fields status,pv,attached_to,used_by,events
```

### k8s pv list

```text
agent-cli k8s pv list — List persistent volumes: claim, capacity, reclaim policy, Azure disk or share
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
  --limit int      (default 50)
Returns: [{id,status,claim,capacity,access_modes[],reclaim,storage_class,driver,disk,share{resource_group,account,name},zone,age}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s pv list --cluster qa --fields id,status,claim,disk
```

### k8s pv get

```text
agent-cli k8s pv get — Show a persistent volume: Azure disk id, zone affinity, attachment, events
 *<pv> str         The volume: its id (cluster/name) or name
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
Returns: {id,status,claim,capacity,access_modes[],reclaim,storage_class,driver,disk,share{resource_group,account,name},zone,age,node_affinity[],attached_to,events[{type,reason,object,message,count,last_seen,namespace}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s pv get qa/pvc-0a1b2c3d-1111-4000-8000-000000000001 --fields disk,zone,attached_to
```

### k8s storageclass list

```text
agent-cli k8s storageclass list — List storage classes: default, provisioner, SKU, binding mode, volume expansion
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
  --limit int      (default 50)
Returns: [{name,default,provisioner,parameters,binding_mode,expansion,reclaim}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s storageclass list --cluster qa --fields name,default,provisioner,expansion
```

### k8s deployment list

```text
agent-cli k8s deployment list — List deployments: ready pods, images with tag and digest, when they rolled out
  <name> str       Part of the deployment name
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
  --since time     Only deployments that rolled out after this
  --image str      Only deployments running this image: repo:tag (api:v1.4.2), a tag alone, a full reference, or a digest (sha256:…)
  --limit int      (default 50)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: [{id,name,namespace,ready,replicas,images[{container,image,digest}],updated}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s deployment list --cluster prod --namespace web --fields id,ready,images,updated
```

### k8s deployment get

```text
agent-cli k8s deployment get — Describe a deployment: revision history, strategy, conditions, env, probes
 *<deployment> str  The deployment: its id (cluster/namespace/name), namespace/name, or name
  --cluster str     The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str   Defaults to the scope's only namespace
Returns: {id,name,namespace,ready,replicas,images[{container,image,digest}],updated,strategy,conditions[{type,status,reason,message}],revisions[{revision,images[],change_cause,created,replicas}],containers[{name,image,env[{name,secret,configmap,key}]}],probes[{container,kind,action,initial_delay,period,failure_threshold}],requests,limits,volumes[{name,kind,source}],events[{type,reason,object,message,count,last_seen,namespace}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s deployment get prod/web/api --fields revisions,conditions,containers
```

### k8s deployment restart

```text
agent-cli k8s deployment restart — Restart a deployment, replacing its pods one at a time
 *<name> str       A deployment's name or id (cluster/namespace/name), or statefulset/NAME, daemonset/NAME
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
Returns: {cluster,namespace,object,said,replicas,previous}
Destructive: needs --yes; --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s deployment restart orders-api --cluster qa --namespace dev
```

### k8s deployment scale

```text
agent-cli k8s deployment scale — Scale a deployment to a number of replicas
 *<name> str       A deployment's name or id (cluster/namespace/name), or statefulset/NAME, replicaset/NAME
 *--replicas int   How many pods it should run
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
Returns: {cluster,namespace,object,said,replicas,previous}
Destructive: needs --yes; --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s deployment scale orders-api --replicas 3 --cluster qa --namespace dev
```

### k8s deployment update

```text
agent-cli k8s deployment update — Roll a deployment back to an earlier version of its pod template
 *<deployment> str  The deployment: its id (cluster/namespace/name), namespace/name, or name
 *--revision int    The revision to roll back to, from deployment get's revisions
  --cluster str     The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str   Defaults to the scope's only namespace
Returns: {id,revision,images[],previous,said}
Destructive: needs --yes; --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s deployment update prod/web/api --revision 6 --yes
```

### k8s deployment wait

```text
agent-cli k8s deployment wait — Wait for a rollout: exit 0 once rolled out, 1 if it failed, 124 if still going
 *<name> str       The deployment: its id (cluster/namespace/name) as `deployment list` prints it, or its name
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
Returns: {id,name,namespace,ready,replicas,images[{container,image,digest}],updated,rolled_out}
Read. * required. Globals: --fields --raw --timeout (default 100s) --output
e.g. agent-cli k8s deployment wait prod/web/api
```

### k8s statefulset list

```text
agent-cli k8s statefulset list — List statefulsets: ready pods, images, volume claim templates
  <name> str       Part of the statefulset name
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
  --limit int      (default 50)
Returns: [{id,name,namespace,ready,images[],claim_templates[],age}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s statefulset list --cluster prod --namespace web --fields id,ready,claim_templates
```

### k8s statefulset get

```text
agent-cli k8s statefulset get — Describe a statefulset: revisions, partition, each pod's volume claim, events
 *<statefulset> str  The statefulset: its id (cluster/namespace/name), namespace/name, or name
  --cluster str      The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str    Defaults to the scope's only namespace
Returns: {id,name,namespace,ready,images[],claim_templates[],age,service,update_strategy,partition,current_revision,update_revision,updated,claims[{template,storage_class,requested,access_modes[],pvcs[]}],events[{type,reason,object,message,count,last_seen,namespace}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s statefulset get prod/web/orders-db --fields ready,claims,events
```

### k8s statefulset restart

```text
agent-cli k8s statefulset restart — Restart a statefulset, replacing its pods one at a time from the highest
 *<statefulset> str  The statefulset: its id (cluster/namespace/name), namespace/name, or name
  --cluster str      The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str    Defaults to the scope's only namespace
Returns: {cluster,namespace,object,said,replicas,previous}
Destructive: needs --yes; --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s statefulset restart prod/web/orders-db --yes
```

### k8s statefulset scale

```text
agent-cli k8s statefulset scale — Scale a statefulset to a number of replicas
 *<statefulset> str  The statefulset: its id (cluster/namespace/name), namespace/name, or name
 *--replicas int     How many pods it should run; fewer removes the highest ordinals, not their claims
  --cluster str      The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str    Defaults to the scope's only namespace
Returns: {cluster,namespace,object,said,replicas,previous}
Destructive: needs --yes; --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s statefulset scale prod/web/orders-db --replicas 0 --yes
```

### k8s daemonset list

```text
agent-cli k8s daemonset list — List daemonsets, kube-system's too: desired, ready, unavailable per node
  <name> str       Part of the daemonset name
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
  --limit int      (default 50)
Returns: [{id,name,namespace,desired,ready,updated,unavailable,node_selector,images[],age}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s daemonset list --cluster prod --namespace kube-system --fields id,desired,ready,unavailable
```

### k8s daemonset get

```text
agent-cli k8s daemonset get — Describe a daemonset, kube-system's too: unready pods, update strategy, events
 *<daemonset> str  The daemonset: its id (cluster/namespace/name), namespace/name, or name
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
Returns: {id,name,namespace,desired,ready,updated,unavailable,node_selector,images[],age,update_strategy,misscheduled,unready[{id,name,namespace,status,ready,restarts,age,node,owner}],events[{type,reason,object,message,count,last_seen,namespace}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s daemonset get prod/kube-system/azure-cns
```

### k8s daemonset restart

```text
agent-cli k8s daemonset restart — Restart a daemonset, replacing its pod on each node in turn
 *<daemonset> str  The daemonset: its id (cluster/namespace/name), namespace/name, or name
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
Returns: {cluster,namespace,object,said,replicas,previous}
Destructive: needs --yes; --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s daemonset restart prod/kube-system/azure-cns --yes
```

### k8s job list

```text
agent-cli k8s job list — List jobs: status, completions, failures, duration, the cronjob that made each
  <name> str       Part of the job name (a CronJob's runs start with its name)
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
  --limit int      (default 50)
Returns: [{id,name,namespace,status,completions,failed,started,duration,cronjob}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s job list --cluster prod --namespace web --fields id,status,started,cronjob
```

### k8s job get

```text
agent-cli k8s job get — Show why a job failed: its tries, conditions, the cronjob behind it, events
 *<job> str        The job: its id (cluster/namespace/name), namespace/name, or name
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
Returns: {id,name,namespace,status,completions,failed,started,duration,cronjob,backoff_limit,deadline,conditions[{type,status,reason,message}],pods[{id,name,namespace,status,ready,restarts,age,node,owner}],events[{type,reason,object,message,count,last_seen,namespace}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s job get prod/web/orders-export-29844060 --fields status,conditions,pods
```

### k8s job delete

```text
agent-cli k8s job delete — Delete a job and its pods, stopping it if it still runs
 *<job> str        The job: its id (cluster/namespace/name), namespace/name, or name
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
Returns: {cluster,namespace,object,said,replicas,previous}
Destructive: needs --yes; --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s job delete prod/web/orders-export-29844060 --yes
```

### k8s cronjob list

```text
agent-cli k8s cronjob list — List cronjobs: schedule, time zone, suspended, last run and last success
  <name> str       Part of the cronjob name
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
  --limit int      (default 50)
Returns: [{id,name,namespace,schedule,time_zone,suspended,last_schedule,last_success,active[]}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s cronjob list --cluster prod --namespace web --fields id,schedule,last_schedule,last_success
```

### k8s cronjob get

```text
agent-cli k8s cronjob get — Describe a cronjob: recent runs with status, why one did not run
 *<cronjob> str    The cronjob: its id (cluster/namespace/name), namespace/name, or name
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
Returns: {id,name,namespace,schedule,time_zone,suspended,last_schedule,last_success,active[],concurrency_policy,starting_deadline,jobs[{id,name,namespace,status,completions,failed,started,duration,cronjob}],events[{type,reason,object,message,count,last_seen,namespace}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s cronjob get prod/web/orders-export --fields schedule,last_success,jobs
```

### k8s cronjob run

```text
agent-cli k8s cronjob run — Run a cronjob now, as a job made from its template, off its schedule
 *<cronjob> str    The cronjob: its id (cluster/namespace/name), namespace/name, or name
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
Returns: {id,cronjob,said}
Write: --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s cronjob run prod/web/orders-export
```

### k8s hpa list

```text
agent-cli k8s hpa list — List autoscalers: min, max, current and desired replicas, metrics vs target
  <name> str       Part of the autoscaler's name (usually its workload's)
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
  --limit int      (default 50)
Returns: [{id,name,namespace,target,min,max,current,desired,metrics[{metric,current,target}],conditions[{type,status,reason,message}]}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s hpa list --cluster prod --namespace web --fields id,current,max,metrics,conditions
```

### k8s pdb list

```text
agent-cli k8s pdb list — List disruption budgets (PDBs): allowed disruptions; 0 blocks drains, upgrades
  <name> str       Part of the budget's name
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
  --limit int      (default 50)
Returns: [{id,name,namespace,selector,min_available,max_unavailable,allowed_disruptions,current_healthy,desired_healthy,expected}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s pdb list --cluster prod --namespace web --fields id,selector,allowed_disruptions
```

### k8s node list

```text
agent-cli k8s node list — List nodes: ready, cordoned, taints, CPU and memory requests and usage
  <name> str       Part of the node name
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
  --limit int      (default 50)
Returns: [{id,name,ready,schedulable,pool,nodepool,zone,version,taints[],cpu{requested,allocatable,requested_percent,usage,usage_percent},memory{requested,allocatable,requested_percent,usage,usage_percent},pressure[],pod_count,age}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s node list --cluster prod --fields id,ready,schedulable,pool,cpu
```

### k8s node get

```text
agent-cli k8s node get — Show a node: conditions, requests, its Azure VM and AKS pool, events
 *<node> str       The node: its id (cluster/name) or name
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
Returns: {id,name,ready,schedulable,pool,nodepool,zone,version,taints[],cpu{requested,allocatable,requested_percent,usage,usage_percent},memory{requested,allocatable,requested_percent,usage,usage_percent},pressure[],pod_count,age,vm,conditions[{type,status,reason,message}],ips[],pod_cidr,capacity,allocatable,pods[{id,status,cpu,memory}],events[{type,reason,object,message,count,last_seen,namespace}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s node get prod/aks-user-27182818-vmss000001 --fields ready,conditions,vm,nodepool
```

### k8s node update

```text
agent-cli k8s node update — Cordon or uncordon a node: whether new pods may be scheduled on it
 *<node> str                The node: its id (cluster/name) or name
 *--schedulable true|false  false cordons it (no new pod lands there; those on it stay), true uncordons it
  --cluster str             The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str           Defaults to the scope's only namespace
Returns: {id,schedulable,said}
Destructive: needs --yes; --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s node update prod/aks-user-27182818-vmss000001 --schedulable false --yes
```

### k8s namespace list

```text
agent-cli k8s namespace list — List namespaces: status, labels, quota used of hard, whether the scope reads it
  <name> str       Part of the namespace's name
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
  --limit int      (default 50)
Returns: [{name,status,labels,quota,in_scope,age}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s namespace list --cluster prod --fields name,status,quota,in_scope
```

### k8s quota list

```text
agent-cli k8s quota list — List resource quotas used of hard, and limit ranges' default requests and limits
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
  --limit int      (default 50)
Returns: [{namespace,from,resource,used,hard,applies_to,default,default_request,min,max}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s quota list --cluster prod --namespace web --fields from,resource,used,hard
```

### k8s serviceaccount list

```text
agent-cli k8s serviceaccount list — List service accounts: their Azure client id, the pods that run as each
  <name> str       Part of the service account's name
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
  --limit int      (default 50)
Returns: [{id,name,namespace,client_id,pull_secrets[],automount,pods[]}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s serviceaccount list --cluster prod --fields id,client_id,pods
```

### k8s serviceaccount get

```text
agent-cli k8s serviceaccount get — Show a service account: its Azure client id, pods, what RBAC lets it do
 *<account> str    The service account: its id (cluster/namespace/name), namespace/name, or name
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
Returns: {id,name,namespace,client_id,pull_secrets[],automount,pods[],can[{resource,names[],verbs[]}],events[{type,reason,object,message,count,last_seen,namespace}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s serviceaccount get prod/web/worker --fields client_id,pods,can
```

### k8s access list

```text
agent-cli k8s access list — List what RBAC allows you or a service account to do in a namespace (auth can-i)
  --as str         Whose access: a serviceaccount's id (cluster/namespace/name), namespace/name or name; @me (the default) is yourself
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
  --limit int      (default 50)
Returns: [{resource,names[],verbs[]}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s access list --as prod/web/worker --fields resource,names,verbs
```

### k8s configmap list

```text
agent-cli k8s configmap list — List configmaps and their keys
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
  --limit int      (default 50)
Returns: [{id,name,namespace,keys[],age}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s configmap list --cluster qa --namespace dev --fields id,keys
```

### k8s configmap get

```text
agent-cli k8s configmap get — Show a configmap's keys and values (binary keys show their size only)
 *<name> str       The configmap: its id (cluster/namespace/name), namespace/name, or name
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
Returns: {id,name,namespace,age,data}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s configmap get qa/dev/orders-config
```

### k8s secret list

```text
agent-cli k8s secret list — List Kubernetes secrets: type, key names and sizes (never values)
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
  --limit int      (default 50)
Returns: [{id,name,namespace,type,keys,age}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s secret list --cluster qa --namespace dev --fields id,type,keys
```

### k8s secret get

```text
agent-cli k8s secret get — Decode one key of a Kubernetes secret (needs --reveal, or --output FILE)
 *<name> str       The secret: its id (cluster/namespace/name), namespace/name, or name
 *<key> str        Which key to decode
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
Returns: {secret,key,namespace,value,encoding}
Reveals a secret: --reveal prints it; --output FILE saves it (0600) and prints only the path. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s secret get orders-db password --cluster qa --namespace dev --fields value --output orders-db.txt
```

### k8s context list

```text
agent-cli k8s context list — List the configured cluster scopes and whether kubectl knows each context
Returns: [{name,context,namespaces[],known}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s context list --fields name,context,namespaces,known
```

### k8s context get

```text
agent-cli k8s context get — Check a cluster: server version, failing readiness checks, DNS address, CNI
  <cluster> str  The [[k8s.scope]] name (or its kube context); defaults to the only one
  --cluster str  The same, as a flag
Returns: {name,context,version,failing[],dns_ip,cni,nodes}
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s context get qa
```

### k8s kind list

```text
agent-cli k8s kind list — List every kind a cluster has, CRDs included: names, short names, API group
  <name> str       Part of the name, kind or API group: cert, gateway, keda.sh
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
  --limit int      (default 50)
Returns: [{name,short_names[],api_version,namespaced,kind,verbs[]}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s kind list cert --cluster qa --fields name,kind,api_version,namespaced
```

### k8s object list

```text
agent-cli k8s object list — List objects of any kind, custom resources (CRDs) too: id, age, ready, status
 *<kind> str       The kind, as `k8s kind list` names it: certificates, cert, Certificate, certificates.cert-manager.io
  <name> str       Part of the object's name
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
  --label str[]    Only objects with this label: app=api, or a key alone (repeatable, all must hold)
  --limit int      (default 50)
Returns: [{id,kind,namespace,age,ready,status}]
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s object list certificates --cluster qa --namespace dev --fields id,ready,status
```

### k8s object get

```text
agent-cli k8s object get — Show one object of any kind, a custom resource (CRD) too: manifest and status
 *<kind> str       The kind, as `k8s kind list` names it: certificates, cert, node
 *<object> str     The object: its id from object list (cluster/namespace/name, or cluster/name for a cluster-scoped kind), namespace/name, or name
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
  --yaml           The manifest as kubectl's YAML text instead
Returns: {id,manifest,text}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s object get certificate qa/dev/orders-tls --fields manifest
```
