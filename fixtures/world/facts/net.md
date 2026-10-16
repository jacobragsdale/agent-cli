# net in the contoso world

What the recordings in `http/azure.json` say, consistent with every other
domain's facts. Keep a new recording consistent with these, and add a line
when you add a fact.

- `vnet-contoso-prod` (rg-contoso-prod, westeurope) is `10.1.0.0/16`, uses
  Azure's DNS, and is peered (`to-hub`, Connected, forwarded traffic allowed)
  with `vnet-contoso-hub` (`10.100.0.0/16`) in the connectivity subscription
  `…5b02`, which `[azure] subscriptions` leaves out.
- Subnet `aks` (`10.1.0.0/22`) holds the cluster's nodes: six IPs used of
  1019, three per pool's scale set (`aks-system-31415926-vmss`,
  `aks-user-27182818-vmss` in the node group), so `net subnet get` names
  `aks-contoso-prod/system` and `/user`. Azure CNI overlay gives pods no
  subnet IPs, so the subnet is far from full.
- Subnet `private-endpoints` (`10.1.4.0/27`): two IPs used of 27, by
  `pe-kv-contoso-billing` and `pe-kv-contoso-prod` (Key Vault's endpoints)
  through their NICs.
- `nsg-contoso-aks`, on the `aks` subnet only: inbound `allow-https-inbound`
  (100, Internet to 443) and `deny-ssh-inbound` (200, Internet to 22), and
  the six defaults.
- `rt-contoso-aks`, on the `aks` subnet, sends `0.0.0.0/0` to the hub
  firewall at `10.100.1.4` (`default-to-firewall`) with BGP propagation off:
  every egress from the nodes, Key Vault's public address included, goes
  through the firewall.
- Node `aks-system-31415926-vmss000001`'s NIC (`…/virtualMachines/1/networkInterfaces/aks-system-31415926-vmss`)
  is `10.1.0.5`, read at `2018-10-01`. Azure refuses its effective rules and
  routes (a stand-in 404 until work shows the real answer), so `net nic get`
  prints the subnet's NSG rules and route table and says so in two notes.
- Subnet `appgw` (`10.1.5.0/24`): two IPs used of 251, by `agw-contoso-prod`.
- `pe-kv-contoso-prod` reaches `kv-contoso-prod` (group `vault`, Approved)
  at `10.1.4.4`, the IP `kv-contoso-prod.vault.azure.net` should resolve to.
  Its zone group keeps the A record `kv-contoso-prod` in the private zone
  `privatelink.vaultcore.azure.net` (rg-contoso-prod), which holds it at
  `10.1.4.4`. The zone is linked to `vnet-contoso-hub` (`link-hub`) and to
  `vnet-contoso-prod` (`link-aks`), so the AKS pods reach the vault
  privately, which trace 4's 403 needs (facts/arm.md); `net endpoint get`
  prints no next step, and `net zone list --vnet vnet-contoso-prod` lists
  the zone.
- Trace 1, private DNS: `pe-kv-contoso-billing` reaches `kv-contoso-billing`
  (Approved) at `10.1.4.5`, but was made without a DNS zone group, so the
  zone holds no `kv-contoso-billing` record. Public DNS CNAMEs
  `kv-contoso-billing.vault.azure.net` into `privatelink.vaultcore.azure.net`,
  and a vnet linked to that zone gets NXDOMAIN: the `billing-sync` pod's
  "Name or service not known" (facts/k8s.md). `net endpoint get` has no
  `dns` and names `net zone list privatelink`; `net record list` on the zone
  shows only `kv-contoso-prod`.
- The public zone `contoso.example` holds NS, SOA, `api` at `203.0.113.10`
  (the app-routing ingress, facts/k8s.md) and `www` at `198.51.100.30` (the
  gateway).
- AKS's load balancer `kubernetes` (node group, Standard) has two frontends:
  the cluster's outbound IP `…e1b1` (`198.51.100.20`, aks cluster get's
  outbound IP) and `a0f6b1c2…`, whose public IP `203.0.113.10` AKS tagged
  `k8s-azure-service: app-routing-system/nginx`, the app-routing ingress
  controller's service (outside the k8s scope's namespaces), with rules and
  probes on 80 and 443. Its pools `kubernetes` and `aksOutboundBackendPool`
  hold the six nodes. `net ip list` lists the three public IPs.
- `agw-contoso-prod` (WAF_v2, Prevention, autoscaled 1 to 3) serves
  `www.contoso.example` on 443 (`www-https`, rule `www`) from pool `web`
  (`web-1.contoso.example`, `web-2.contoso.example`) over `web-https` with
  probe `web-health` (`/healthz`, 200-399). `--health` finds web-1 Healthy
  and web-2 Unhealthy: it answers the probe with 502.
- `arm resource list` lists the NSG, route table, both endpoints and their
  NICs, the two zones and the private zone's two links, the load balancer,
  the three public IPs and the gateway beside the earlier six; the node
  group's scale sets are not in it.
