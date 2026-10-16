# net — Azure networking (16 commands)

| Command | Effect | Summary |
|---|---|---|
| [`net vnet list`](#net-vnet-list) | read | List virtual networks with their address space, subnets, peerings and DNS |
| [`net vnet get`](#net-vnet-get) | read | Show a vnet's subnets with their used and free IPs, NSGs and route tables |
| [`net subnet get`](#net-subnet-get) | read | Show a subnet's used and free IPs, who holds them, its NSG and route table |
| [`net nsg list`](#net-nsg-list) | read | List network security groups with their rule counts and what they guard |
| [`net nsg get`](#net-nsg-get) | read | Show an NSG's inbound and outbound rules by priority, defaults included |
| [`net nic get`](#net-nic-get) | read | Show a network interface's IPs and the security rules and routes in effect |
| [`net route list`](#net-route-list) | read | List route tables and their routes, next hops, subnets and BGP propagation |
| [`net endpoint list`](#net-endpoint-list) | read | List private endpoints with what they reach, their status and private IPs |
| [`net endpoint get`](#net-endpoint-get) | read | Show a private endpoint's IPs and whether its name resolves to it from its vnet |
| [`net zone list`](#net-zone-list) | read | List public and private DNS zones and the vnets each private zone is linked to |
| [`net record list`](#net-record-list) | read | List a DNS zone's records with their type, TTL and values, public or private |
| [`net ip list`](#net-ip-list) | read | List public IPs with what holds each and the k8s service it serves |
| [`net lb list`](#net-lb-list) | read | List load balancers with their counts of frontends, rules and backend pools |
| [`net lb get`](#net-lb-get) | read | Show a load balancer's frontends and their k8s services, rules, probes and pools |
| [`net appgw list`](#net-appgw-list) | read | List application gateways with their SKU, status and WAF mode |
| [`net appgw get`](#net-appgw-get) | read | Show an application gateway's routing rules, backends, probes and WAF mode |

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

### net endpoint list

```text
agent-cli net endpoint list — List private endpoints with what they reach, their status and private IPs
  --group str         Its resource group: where a name is looked for, or the one a list keeps to
  --subscription str  Its subscription, GUID or name (within [azure] subscriptions)
  --limit int         Most rows to return (default 50)
Returns: [{id,name,group,target,group_ids[],status,subnet,ips[]}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli net endpoint list --fields id,target,status,ips
```

### net endpoint get

```text
agent-cli net endpoint get — Show a private endpoint's IPs and whether its name resolves to it from its vnet
 *<endpoint> str      The private endpoint: its ARM id, portal link, or name
  --group str         Its resource group: where a name is looked for, or the one a list keeps to
  --subscription str  Its subscription, GUID or name (within [azure] subscriptions)
Returns: {id,name,target,group_ids[],status,subnet,vnet,ips[],fqdns[],dns[{fqdn,zone,record[],resolves,linked}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli net endpoint get pe-kv-contoso-prod --fields ips,dns
```

### net zone list

```text
agent-cli net zone list — List public and private DNS zones and the vnets each private zone is linked to
  <name> str          Part of the zone's name: privatelink, vaultcore
  --vnet str          Only the private zones linked to this vnet: its ARM id, portal link, or name
  --group str         Its resource group: where a name is looked for, or the one a list keeps to
  --subscription str  Its subscription, GUID or name (within [azure] subscriptions)
  --limit int         Most rows to return (default 50)
Returns: [{id,name,group,private,records,links[{vnet,registration,state}]}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli net zone list privatelink --fields name,links
```

### net record list

```text
agent-cli net record list — List a DNS zone's records with their type, TTL and values, public or private
 *<zone> str          The DNS zone, public or private: its ARM id, portal link, or name
  <name> str          Part of the record's name: kv-contoso, api
  --group str         Its resource group: where a name is looked for, or the one a list keeps to
  --subscription str  Its subscription, GUID or name (within [azure] subscriptions)
  --limit int         Most rows to return (default 50)
Returns: [{name,type,ttl,values[]}]
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli net record list privatelink.vaultcore.azure.net --fields name,type,values
```

### net ip list

```text
agent-cli net ip list — List public IPs with what holds each and the k8s service it serves
  --group str         Its resource group: where a name is looked for, or the one a list keeps to
  --subscription str  Its subscription, GUID or name (within [azure] subscriptions)
  --limit int         Most rows to return (default 50)
Returns: [{id,name,group,address,sku,allocation,held_by,service}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli net ip list --fields address,held_by,service
```

### net lb list

```text
agent-cli net lb list — List load balancers with their counts of frontends, rules and backend pools
  --group str         Its resource group: where a name is looked for, or the one a list keeps to
  --subscription str  Its subscription, GUID or name (within [azure] subscriptions)
  --limit int         Most rows to return (default 50)
Returns: [{id,name,group,location,sku,frontends,rules,pools}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli net lb list --fields id,frontends,rules
```

### net lb get

```text
agent-cli net lb get — Show a load balancer's frontends and their k8s services, rules, probes and pools
 *<lb> str            The load balancer: its ARM id, portal link, or name
  --group str         Its resource group: where a name is looked for, or the one a list keeps to
  --subscription str  Its subscription, GUID or name (within [azure] subscriptions)
Returns: {id,name,group,sku,frontends[{name,private_ip,subnet,public_ip,address,service,service_uid}],rules[{name,frontend,pool,protocol,frontend_port,backend_port,probe}],probes[{name,protocol,port,path,interval,threshold}],pools[{name,members}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli net lb get kubernetes --group MC_rg-contoso-prod_aks-contoso-prod_westeurope --fields frontends
```

### net appgw list

```text
agent-cli net appgw list — List application gateways with their SKU, status and WAF mode
  --group str         Its resource group: where a name is looked for, or the one a list keeps to
  --subscription str  Its subscription, GUID or name (within [azure] subscriptions)
  --limit int         Most rows to return (default 50)
Returns: [{id,name,group,location,sku,status,waf_mode,waf_policy,listeners,pools}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli net appgw list --fields id,sku,status,waf_mode
```

### net appgw get

```text
agent-cli net appgw get — Show an application gateway's routing rules, backends, probes and WAF mode
 *<appgw> str         The application gateway: its ARM id, portal link, or name
  --health            Also ask Azure how each backend answers its probe (takes up to a minute)
  --group str         Its resource group: where a name is looked for, or the one a list keeps to
  --subscription str  Its subscription, GUID or name (within [azure] subscriptions)
Returns: {id,name,group,sku,status,capacity,subnet,waf{mode,enabled,policy},frontends[{name,private_ip,public_ip}],listeners[{name,frontend,port,protocol,hosts[],certificate}],rules[{name,priority,kind,listener,pool,settings,path_map,redirect}],pools[{name,addresses[]}],settings[{name,port,protocol,host,backend_host,probe,timeout}],probes[{name,protocol,host,path,interval,timeout,threshold,statuses[]}],health[{pool,settings,address,health,log}]}
Read. * required. Globals: --fields --raw --timeout (default 100s) --output
e.g. agent-cli net appgw get agw-contoso-prod --health --fields rules,health
```
