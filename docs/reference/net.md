# net — Azure networking (7 commands)

| Command | Effect | Summary |
|---|---|---|
| [`net vnet list`](#net-vnet-list) | read | List virtual networks with their address space, subnets, peerings and DNS |
| [`net vnet get`](#net-vnet-get) | read | Show a vnet's subnets with their used and free IPs, NSGs and route tables |
| [`net subnet get`](#net-subnet-get) | read | Show a subnet's used and free IPs, who holds them, its NSG and route table |
| [`net nsg list`](#net-nsg-list) | read | List network security groups with their rule counts and what they guard |
| [`net nsg get`](#net-nsg-get) | read | Show an NSG's inbound and outbound rules by priority, defaults included |
| [`net nic get`](#net-nic-get) | read | Show a network interface's IPs and the security rules and routes in effect |
| [`net route list`](#net-route-list) | read | List route tables and their routes, next hops, subnets and BGP propagation |

### net vnet list

```text
agent-cli net vnet list — List virtual networks with their address space, subnets, peerings and DNS
  --group str         Its resource group: where a name is looked for, or the one a list keeps to
  --subscription str  Its subscription, GUID or name (within [azure] subscriptions)
  --limit int         Most rows to return (default 50)
Returns: [{id,name,group,location,address_space[],subnets[{name,prefix}],peerings[{name,remote,state,sync,forwarded_traffic,gateway_transit,remote_gateways}],dns_servers[]}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli net vnet list --fields id,address_space,peerings
```

### net vnet get

```text
agent-cli net vnet get — Show a vnet's subnets with their used and free IPs, NSGs and route tables
 *<vnet> str          The vnet: its ARM id, portal link, or name
  --group str         Its resource group: where a name is looked for, or the one a list keeps to
  --subscription str  Its subscription, GUID or name (within [azure] subscriptions)
Returns: {id,name,location,address_space[],dns_servers[],subnets[{id,name,prefix,used_ips,free_ips,nsg,route_table,nat_gateway,delegations[]}],peerings[{name,remote,state,sync,forwarded_traffic,gateway_transit,remote_gateways}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli net vnet get vnet-contoso-prod --fields subnets
```

### net subnet get

```text
agent-cli net subnet get — Show a subnet's used and free IPs, who holds them, its NSG and route table
 *<subnet> str        The subnet: its ARM id, portal link, or VNET/SUBNET
  --group str         Its resource group: where a name is looked for, or the one a list keeps to
  --subscription str  Its subscription, GUID or name (within [azure] subscriptions)
Returns: {id,name,vnet,prefix,used_ips,free_ips,nsg,route_table,nat_gateway,service_endpoints[],delegations[],private_endpoints[],holders[{id,type,ips,pool}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli net subnet get vnet-contoso-prod/aks --group rg-contoso-prod
```

### net nsg list

```text
agent-cli net nsg list — List network security groups with their rule counts and what they guard
  --group str         Its resource group: where a name is looked for, or the one a list keeps to
  --subscription str  Its subscription, GUID or name (within [azure] subscriptions)
  --limit int         Most rows to return (default 50)
Returns: [{id,name,group,location,rules,subnets[],nics[]}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli net nsg list --fields id,rules,subnets
```

### net nsg get

```text
agent-cli net nsg get — Show an NSG's inbound and outbound rules by priority, defaults included
 *<nsg> str           The NSG: its ARM id, portal link, or name
  --group str         Its resource group: where a name is looked for, or the one a list keeps to
  --subscription str  Its subscription, GUID or name (within [azure] subscriptions)
Returns: {id,name,location,subnets[],nics[],rules[{direction,priority,name,access,protocol,source[],source_ports[],destination[],destination_ports[],default,nsg}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli net nsg get nsg-contoso-aks --fields rules
```

### net nic get

```text
agent-cli net nic get — Show a network interface's IPs and the security rules and routes in effect
 *<nic> str           The NIC: its ARM id (a scale set instance's too, as an AKS node's), portal link, or name
  --group str         Its resource group: where a name is looked for, or the one a list keeps to
  --subscription str  Its subscription, GUID or name (within [azure] subscriptions)
Returns: {id,name,vm,private_endpoint,subnet,nsg,ips[{name,private_ip,public_ip,primary}],effective_rules[{direction,priority,name,access,protocol,source[],source_ports[],destination[],destination_ports[],default,nsg}],effective_routes[{name,source,state,prefixes[],next_hop,next_hop_ips[]}]}
Read. * required. Globals: --fields --raw --timeout (default 100s) --output
e.g. agent-cli net nic get nic-contoso-jump --fields ips,effective_rules
```

### net route list

```text
agent-cli net route list — List route tables and their routes, next hops, subnets and BGP propagation
  --group str         Its resource group: where a name is looked for, or the one a list keeps to
  --subscription str  Its subscription, GUID or name (within [azure] subscriptions)
  --limit int         Most rows to return (default 50)
Returns: [{id,table,route,prefix,next_hop,next_hop_ip,bgp_propagation,subnets[]}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli net route list --fields table,prefix,next_hop,next_hop_ip,subnets
```
