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
- Subnet `private-endpoints` (`10.1.4.0/27`): one IP used of 27, by
  `pe-kv-contoso-prod` (Key Vault's endpoint) through its NIC.
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
- `arm resource list` lists the NSG, route table, endpoint and its NIC
  beside the earlier six; the node group's scale sets are not in it.
