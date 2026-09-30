# k8s — Kubernetes (13 commands)

| Command | Effect | Summary |
|---|---|---|
| [`k8s pod list`](#k8s-pod-list) | read | List pods with status, ready, restarts, age, node and owning deployment |
| [`k8s pod get`](#k8s-pod-get) | read | Describe a pod: containers, images, states, last termination reason, owner |
| [`k8s pod logs`](#k8s-pod-logs) | read | Read the tail of a pod's log, or of the container's previous run |
| [`k8s pod delete`](#k8s-pod-delete) | destructive | Delete a pod so its controller replaces it (a bare pod is gone for good) |
| [`k8s event list`](#k8s-event-list) | read | List Kubernetes events, newest first: warnings, back-offs, failed pulls |
| [`k8s deployment list`](#k8s-deployment-list) | read | List deployments: ready pods, images with tag and digest, when they rolled out |
| [`k8s deployment restart`](#k8s-deployment-restart) | destructive | Rollout-restart a deployment, replacing its pods one at a time |
| [`k8s deployment scale`](#k8s-deployment-scale) | destructive | Scale a deployment to a number of replicas |
| [`k8s configmap list`](#k8s-configmap-list) | read | List configmaps and their keys |
| [`k8s configmap get`](#k8s-configmap-get) | read | Show a configmap's keys and values (binary keys show their size only) |
| [`k8s secret list`](#k8s-secret-list) | read | List Kubernetes secrets: type, key names and sizes (never values) |
| [`k8s secret get`](#k8s-secret-get) | reveal | Decode one key of a Kubernetes secret (needs --reveal, or --output FILE) |
| [`k8s context list`](#k8s-context-list) | read | List the configured cluster scopes and whether kubectl knows each context |

### k8s pod list

```text
agent-cli k8s pod list — List pods with status, ready, restarts, age, node and owning deployment
  <name> str       Part of the pod name
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
  --label str[]    Only pods with this label: app=api, or a key alone (repeatable, all must hold)
  --limit int      (default 50)
Returns: [{id,name,namespace,status,ready,restarts,age,node,owner}]
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
Returns: {id,name,namespace,status,ready,restarts,age,node,ip,owner,containers[{name,image,digest,ready,restarts,state,last_termination}],secret_refs[{via,secret,keys[],class,kv[]}],conditions[{type,status,reason,message}],labels,text}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s pod get qa/dev/orders-api-7d9f5b-abc12 --fields status,containers,secret_refs
```

### k8s pod logs

```text
agent-cli k8s pod logs — Read the tail of a pod's log, or of the container's previous run
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
  --since time     Only events last seen after this
  --limit int      (default 50)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: [{type,reason,object,message,count,last_seen,namespace}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s event list --pod qa/dev/orders-worker-5c4d3e-q8zt --since 1h --fields type,reason,message,last_seen
```

### k8s deployment list

```text
agent-cli k8s deployment list — List deployments: ready pods, images with tag and digest, when they rolled out
  <name> str       Part of the deployment name
  --cluster str    The [[k8s.scope]] name (or its kube context); defaults to the only one
  --namespace str  Defaults to the scope's only namespace
  --since time     Only deployments that rolled out after this
  --limit int      (default 50)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: [{id,name,namespace,ready,replicas,images[{container,image,digest}],updated}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli k8s deployment list --cluster prod --namespace web --fields id,ready,images,updated
```

### k8s deployment restart

```text
agent-cli k8s deployment restart — Rollout-restart a deployment, replacing its pods one at a time
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
