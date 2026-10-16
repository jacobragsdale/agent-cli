# aks — AKS (8 commands)

| Command | Effect | Summary |
|---|---|---|
| [`aks cluster list`](#aks-cluster-list) | read | List the AKS clusters the az login reaches, with version and power state |
| [`aks cluster get`](#aks-cluster-get) | read | Show an AKS cluster: network, private API server, identity, addons, auto-upgrade |
| [`aks cluster connect`](#aks-cluster-connect) | write | Fetch an AKS cluster's kubeconfig credentials and print its [[k8s.scope]] |
| [`aks nodepool list`](#aks-nodepool-list) | read | List an AKS cluster's node pools: VM size, node count, autoscaler bounds, subnet |
| [`aks nodepool get`](#aks-nodepool-get) | read | Show an AKS node pool: autoscaler, surge settings, newest node image available |
| [`aks nodepool scale`](#aks-nodepool-scale) | destructive | Set the node count of an AKS node pool whose autoscaler is off |
| [`aks upgrade list`](#aks-upgrade-list) | read | List the Kubernetes versions and node images an AKS cluster can upgrade to |
| [`aks command run`](#aks-command-run) | destructive | Run kubectl or helm inside an AKS cluster through ARM, for a private API server |

### aks cluster list

```text
agent-cli aks cluster list — List the AKS clusters the az login reaches, with version and power state
  --limit int  (default 50)
Returns: [{name,resource_group,subscription,location,kubernetes_version,power_state,k8s_scope}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli aks cluster list --fields name,k8s_scope,power_state
```

### aks cluster get

```text
agent-cli aks cluster get — Show an AKS cluster: network, private API server, identity, addons, auto-upgrade
 *<cluster> str         The cluster: its name as `aks cluster list` shows it, its ARM id, or its portal link
  --resource-group str  Its resource group, for a name two groups hold
  --subscription str    Its subscription id, for a name two subscriptions hold
Returns: {name,resource_group,subscription,location,resource,kubernetes_version,tier,power_state,provisioning_state,network{plugin,mode,policy,dataplane,pod_cidr,service_cidr,dns_service_ip,outbound_type,outbound_ips[]},api_server{private,fqdn,authorized_ranges[]},oidc_issuer,workload_identity,identity,addons[],auto_upgrade,node_os_upgrade,maintenance[{name,schedule,start_time,utc_offset,duration_hours,time_in_week}],node_resource_group,k8s_scope}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli aks cluster get aks-contoso-prod --fields network,api_server,identity
```

### aks cluster connect

```text
agent-cli aks cluster connect — Fetch an AKS cluster's kubeconfig credentials and print its [[k8s.scope]]
 *<name> str            The cluster's name, as `aks cluster list` shows it
  --resource-group str  Its resource group; looked up by name when left out
  --subscription str    Its subscription id; the az default, or looked up with the group
Returns: {cluster,resource_group,subscription,context,kubelogin,namespaces[],config}
Write: --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli aks cluster connect aks-contoso-dev --resource-group rg-contoso --fields context,config
```

### aks nodepool list

```text
agent-cli aks nodepool list — List an AKS cluster's node pools: VM size, node count, autoscaler bounds, subnet
 *<cluster> str         The cluster: its name as `aks cluster list` shows it, its ARM id, or its portal link
  --resource-group str  Its resource group, for a name two groups hold
  --subscription str    Its subscription id, for a name two subscriptions hold
  --limit int           Most rows to return (default 50)
Returns: [{id,mode,vm_size,count,autoscale{min,max},version,node_image,os_sku,zones[],subnet,max_pods,taints[],labels,state,power_state}]
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli aks nodepool list aks-contoso-prod --fields id,mode,count,autoscale,state
```

### aks nodepool get

```text
agent-cli aks nodepool get — Show an AKS node pool: autoscaler, surge settings, newest node image available
 *<pool> str            The pool: CLUSTER/POOL as `aks nodepool list` prints it, its ARM id, or its portal link
  --resource-group str  Its resource group, for a name two groups hold
  --subscription str    Its subscription id, for a name two subscriptions hold
Returns: {id,mode,vm_size,count,autoscale{min,max},version,node_image,os_sku,zones[],subnet,max_pods,taints[],labels,state,power_state,max_surge,max_unavailable,drain_timeout_minutes,upgrades[],newest_node_image}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli aks nodepool get aks-contoso-prod/user --fields count,autoscale,newest_node_image
```

### aks nodepool scale

```text
agent-cli aks nodepool scale — Set the node count of an AKS node pool whose autoscaler is off
 *<pool> str            The pool: CLUSTER/POOL as `aks nodepool list` prints it, its ARM id, or its portal link
 *--nodes int           How many nodes it should have
  --resource-group str  Its resource group, for a name two groups hold
  --subscription str    Its subscription id, for a name two subscriptions hold
Returns: {id,previous,count,state}
Destructive: needs --yes; --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli aks nodepool scale aks-contoso-prod/system --nodes 4 --yes
```

### aks upgrade list

```text
agent-cli aks upgrade list — List the Kubernetes versions and node images an AKS cluster can upgrade to
 *<cluster> str         The cluster: its name as `aks cluster list` shows it, its ARM id, or its portal link
  --resource-group str  Its resource group, for a name two groups hold
  --subscription str    Its subscription id, for a name two subscriptions hold
  --limit int           Most rows to return (default 50)
Returns: [{target,version,upgrades[],node_image,newest_node_image}]
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli aks upgrade list aks-contoso-prod --fields target,version,upgrades,newest_node_image
```

### aks command run

```text
agent-cli aks command run — Run kubectl or helm inside an AKS cluster through ARM, for a private API server
 *<cluster> str         The cluster: its name as `aks cluster list` shows it, its ARM id, or its portal link
 *<argv> str[]          The command after `--`, such as kubectl get pods -A
  --resource-group str  Its resource group, for a name two groups hold
  --subscription str    Its subscription id, for a name two subscriptions hold
  --tail int            How many of the output's last lines to keep (default 200)
Returns: {id,cluster,command,state,exit_code,reason,started,finished,logs}
Destructive: needs --yes; --dry-run shows the change without making it. * required. Globals: --fields --raw --timeout (default 100s) --output
e.g. agent-cli aks command run aks-contoso-prod --yes -- kubectl get pods -A
```
