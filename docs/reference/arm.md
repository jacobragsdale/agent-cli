# arm — Azure Resource Manager (5 commands)

| Command | Effect | Summary |
|---|---|---|
| [`arm resource list`](#arm-resource-list) | read | List Azure resources of any type by name, type, group, tag or region |
| [`arm resource get`](#arm-resource-get) | read | Show any Azure resource as ARM returns it, by id, portal link or name |
| [`arm query run`](#arm-query-run) | read | Run a Resource Graph (KQL) query across the subscriptions in scope |
| [`arm group list`](#arm-group-list) | read | List Azure resource groups across the subscriptions in scope, with their tags |
| [`arm subscription list`](#arm-subscription-list) | read | List the Azure subscriptions the az login reaches (within [azure] subscriptions) |

### arm resource list

```text
agent-cli arm resource list — List Azure resources of any type by name, type, group, tag or region
  <name> str          Part of the resource name, any case
  --type str          Only this type, any case: Microsoft.Network/virtualNetworks
  --group str         Only this resource group
  --tag str[]         Only resources tagged key, or key=value (value any case; repeatable)
  --location str      Only this region: westeurope
  --subscription str  Only this subscription: its GUID or name (within [azure] subscriptions)
  --limit int         (default 50)
Returns: [{id,name,type,group,location,sku,provisioning_state}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli arm resource list --type Microsoft.Network/virtualNetworks --fields id,location
```

### arm resource get

```text
agent-cli arm resource get — Show any Azure resource as ARM returns it, by id, portal link or name
 *<resource> str      The resource: its ARM id (a child collection such as SERVER_ID/firewallRules too), its portal link, or its name
  --type str          Its type, for a name: Microsoft.Network/virtualNetworks
  --group str         Its resource group, for a name two groups hold
  --subscription str  Its subscription, GUID or name, for a name two subscriptions hold
Returns: any
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli arm resource get vnet-contoso-prod --type Microsoft.Network/virtualNetworks --fields properties.subnets
```

### arm query run

```text
agent-cli arm query run — Run a Resource Graph (KQL) query across the subscriptions in scope
  <kql> str        The KQL, or - to read it from stdin. Resource Graph pages only a query that projects id
  --kql-file path  The KQL from a file
  --limit int      (default 50)
Returns: [any]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli arm query run 'resources | summarize count() by type'
```

### arm group list

```text
agent-cli arm group list — List Azure resource groups across the subscriptions in scope, with their tags
  --subscription str  Only this subscription: its GUID or name (within [azure] subscriptions)
  --limit int         (default 50)
Returns: [{id,name,subscription,location,tags}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli arm group list --fields id,location,tags
```

### arm subscription list

```text
agent-cli arm subscription list — List the Azure subscriptions the az login reaches (within [azure] subscriptions)
  --limit int  (default 50)
Returns: [{id,name,state,tenant}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli arm subscription list --fields id,name,state
```
