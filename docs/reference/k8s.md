# k8s — Kubernetes (24 commands)

| Command | Effect | Summary |
|---|---|---|
| [`k8s pod list`](#k8s-pod-list) | read | List pods: status, ready, restarts, owner; or those mounting a Key Vault secret |
| [`k8s pod get`](#k8s-pod-get) | read | Describe a pod: containers, images, states, last termination reason, owner |
| [`k8s pod logs`](#k8s-pod-logs) | read | Read the tail of a pod's log, or of the container's previous run |
| [`k8s pod delete`](#k8s-pod-delete) | destructive | Delete a pod so its controller replaces it (a bare pod is gone for good) |
| [`k8s event list`](#k8s-event-list) | read | List Kubernetes events, newest first: warnings, back-offs, failed pulls |
| [`k8s service list`](#k8s-service-list) | read | List Kubernetes services: type, cluster IP, external IP, ports, ready endpoints |
| [`k8s service get`](#k8s-service-get) | read | Show a Kubernetes service: ports, endpoint pods and IPs, or what leaves it empty |
| [`k8s ingress list`](#k8s-ingress-list) | read | List ingresses: the hosts each serves, class, TLS secrets, address |
| [`k8s ingress get`](#k8s-ingress-get) | read | Show an ingress: each host and path's service and ready endpoints, TLS secrets |
| [`k8s networkpolicy list`](#k8s-networkpolicy-list) | read | List network policies, or those that apply to one pod: selector, default deny |
| [`k8s networkpolicy get`](#k8s-networkpolicy-get) | read | Show a network policy's rules in words: peers, ports, and the pods it selects |
| [`k8s deployment list`](#k8s-deployment-list) | read | List deployments: ready pods, images with tag and digest, when they rolled out |
| [`k8s deployment restart`](#k8s-deployment-restart) | destructive | Restart a deployment, replacing its pods one at a time |
| [`k8s deployment scale`](#k8s-deployment-scale) | destructive | Scale a deployment to a number of replicas |
| [`k8s deployment wait`](#k8s-deployment-wait) | read | Wait for a rollout: exit 0 once rolled out, 1 if it failed, 124 if still going |
| [`k8s configmap list`](#k8s-configmap-list) | read | List configmaps and their keys |
| [`k8s configmap get`](#k8s-configmap-get) | read | Show a configmap's keys and values (binary keys show their size only) |
| [`k8s secret list`](#k8s-secret-list) | read | List Kubernetes secrets: type, key names and sizes (never values) |
| [`k8s secret get`](#k8s-secret-get) | reveal | Decode one key of a Kubernetes secret (needs --reveal, or --output FILE) |
| [`k8s context list`](#k8s-context-list) | read | List the configured cluster scopes and whether kubectl knows each context |
| [`k8s context get`](#k8s-context-get) | read | Check a cluster: server version, failing readiness checks, DNS address, CNI |
| [`k8s kind list`](#k8s-kind-list) | read | List the kinds a cluster serves, CRDs included: names, short names, API group |
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
Returns: {id,name,namespace,status,ready,restarts,age,node,ip,ips[],host_network,dns_policy,service_account,owner,containers[{name,image,digest,ready,restarts,state,last_termination}],ports[{container,name,port,protocol}],probes[{container,kind,action,initial_delay,period,failure_threshold}],requests,limits,volumes[{name,kind,source}],node_selector,tolerations[],secret_refs[{via,secret,keys[],class,kv[]}],conditions[{type,status,reason,message}],events[{type,reason,object,message,count,last_seen,namespace}],labels,text}
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
agent-cli k8s kind list — List the kinds a cluster serves, CRDs included: names, short names, API group
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
