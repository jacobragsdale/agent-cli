# arm — Azure Resource Manager (13 commands)

| Command | Effect | Summary |
|---|---|---|
| [`arm resource list`](#arm-resource-list) | read | List Azure resources of any type by name, type, group, tag or region |
| [`arm resource get`](#arm-resource-get) | read | Show any Azure resource as ARM returns it, by id, portal link or name |
| [`arm resource diff`](#arm-resource-diff) | read | Compare two Azure resources of any type: NSG rules, network rules, node pools |
| [`arm query run`](#arm-query-run) | read | Run a Resource Graph (KQL) query across the subscriptions in scope |
| [`arm activity list`](#arm-activity-list) | read | List the Azure activity log: who changed or deleted what, and why it failed |
| [`arm change list`](#arm-change-list) | read | List what changed on Azure resources: who, when, each property before and after |
| [`arm metric list`](#arm-metric-list) | read | Get Azure Monitor metric points for any Azure resource, or list its metrics |
| [`arm role list`](#arm-role-list) | read | List Azure role assignments (RBAC) for a principal or applying at a scope |
| [`arm identity list`](#arm-identity-list) | read | List user-assigned managed identities with their client and principal ids |
| [`arm identity get`](#arm-identity-get) | read | Show a managed identity: client id, federated credentials (k8s subject), roles |
| [`arm policy list`](#arm-policy-list) | read | List Azure Policy violations: non-compliant resources, the policy, effect, why |
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

### arm resource diff

```text
agent-cli arm resource diff — Compare two Azure resources of any type: NSG rules, network rules, node pools
 *<left> str      One side: an ARM id, its portal link, or its name
 *<right> str     The other side, as left
  --type str      Their type, for names: Microsoft.Network/networkSecurityGroups
  --as str[]      Treat a word on the left as one on the right, A=B (repeatable); none turns off the names inferred from the two ids
  --ignore str[]  Leave out a path as the changes print it, and all under it (repeatable; * is one segment)
  --all           Keep what this resource's diff leaves out as noise (ids, times, counts)
  --limit int     Most differences to print (default 50)
  --exact         Texts compare as they are: whitespace, case and comments count
Returns: {left,right,same,as[],changes[{path,left,right,redacted,same,lines[]}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli arm resource diff nsg-contoso-staging nsg-contoso-prod --type Microsoft.Network/networkSecurityGroups
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

### arm activity list

```text
agent-cli arm activity list — List the Azure activity log: who changed or deleted what, and why it failed
  --resource str                Only what was done to this resource: an ARM id, portal link or name
  --group str                   Only what was done in this resource group
  --caller str                  Only what this caller did: a UPN, an object id, or @me (the az login)
  --status succeeded|failed|started|accepted|canceled  Only events in this state
  --since time                  From when (default a day before --until; the log keeps 90 days)
  --until time                  Until when (default now)
  --limit int                   (default 50)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: [{when,caller,operation,status,resource,error}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli arm activity list --group rg-contoso-prod --since 2d --fields when,caller,operation,status
```

### arm change list

```text
agent-cli arm change list — List what changed on Azure resources: who, when, each property before and after
  --resource str  Only changes to this resource and what sits under it (its role assignments too): an ARM id, portal link or name
  --group str     Only changes in this resource group
  --since time    From when (default 14 days ago, all Resource Graph keeps)
  --limit int     (default 50)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: [{when,resource,change_type,changed_by,properties[{path,before,after}]}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli arm change list --resource kv-contoso-prod --since 2d --fields when,resource,change_type,changed_by
```

### arm metric list

```text
agent-cli arm metric list — Get Azure Monitor metric points for any Azure resource, or list its metrics
 *<resource> str                The resource: its ARM id (k8s pvc get's disk, k8s node get's vm), portal link or name
  --metric str[]                A metric to read, as the list without it names it: "Composite Disk Read Operations/sec" (repeatable)
  --group str                   Its resource group, for a name two groups hold
  --since time                  From when (default 1h before --until)
  --until time                  Until when (default now)
  --interval duration           One point per this much: 1m, 5m, 15m, 1h, 1d (default Azure's for the window)
  --aggregation average|minimum|maximum|total|count  How a point combines the samples in it (default the metric's own)
  --limit int                   The newest points per metric, or the definitions, to return (default 50)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
A duration is 500ms, 30s, 15m, 2h, 7d, 1w.
Returns: [{metric,unit,aggregations[],points[{at,value}]}]
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli arm metric list pvc-7c3e9a1b-4d2f-4e8a-9b6c-1f0e2d3c4b5a --metric 'Composite Disk Write Operations/sec' --since 3h --interval 5m --fields metric,points
```

### arm role list

```text
agent-cli arm role list — List Azure role assignments (RBAC) for a principal or applying at a scope
  --principal str  Only this principal's: a user, group, service principal or managed identity's object id (arm identity get prints it as principal_id)
  --scope str      Only those that apply here, inherited from its group and subscription too: an ARM id, portal link or resource name
  --limit int      (default 50)
Returns: [{id,principal_id,principal_name,principal_type,role,scope,inherited}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli arm role list --scope kv-contoso-prod --fields principal_id,role,inherited
```

### arm identity list

```text
agent-cli arm identity list — List user-assigned managed identities with their client and principal ids
  --group str         Its resource group: where a name is looked for, or the one a list keeps to
  --subscription str  Its subscription, GUID or name (within [azure] subscriptions)
  --limit int         (default 50)
Returns: [{id,name,group,client_id,principal_id}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli arm identity list --group rg-contoso-prod --fields id,client_id
```

### arm identity get

```text
agent-cli arm identity get — Show a managed identity: client id, federated credentials (k8s subject), roles
 *<identity> str      The identity: its ARM id, portal link, name, or client id (as k8s serviceaccount get prints it)
  --group str         Its resource group: where a name is looked for, or the one a list keeps to
  --subscription str  Its subscription, GUID or name (within [azure] subscriptions)
Returns: {id,name,group,client_id,principal_id,tenant_id,federated_credentials[{name,issuer,subject,serviceaccount}],roles[{id,principal_id,principal_type,role,scope,inherited}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli arm identity get 00000000-0000-4000-8000-00000000b0f1 --fields federated_credentials,roles
```

### arm policy list

```text
agent-cli arm policy list — List Azure Policy violations: non-compliant resources, the policy, effect, why
  --resource str  Only this resource's: an ARM id, portal link or name
  --group str     Only resources in this resource group
  --limit int     (default 50)
Returns: [{resource,policy,display_name,effect,reason,assignment}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli arm policy list --group rg-contoso-prod --fields resource,display_name,effect
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
