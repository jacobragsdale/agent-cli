# aks — AKS (2 commands)

| Command | Effect | Summary |
|---|---|---|
| [`aks cluster list`](#aks-cluster-list) | read | List the AKS clusters the az login reaches, with version and power state |
| [`aks cluster connect`](#aks-cluster-connect) | write | Fetch an AKS cluster's kubeconfig credentials and print its [[k8s.scope]] |

### aks cluster list

```text
agent-cli aks cluster list — List the AKS clusters the az login reaches, with version and power state
  --limit int  (default 50)
Returns: [{name,resource_group,subscription,location,kubernetes_version,power_state,k8s_scope}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli aks cluster list --fields name,k8s_scope,power_state
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
