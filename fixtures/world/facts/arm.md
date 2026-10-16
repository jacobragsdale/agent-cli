# arm in the contoso world

What the recordings in `http/azure.json` say, consistent with every other
domain's facts. Keep a new recording consistent with these, and add a line
when you add a fact.

- One subscription, `contoso-prod` (`00000000-0000-4000-8000-000000005b01`,
  Enabled), the one `[azure] subscriptions` names.
- Resource groups `rg-contoso-prod` (tags `env=prod`, `owner=platform`),
  `rg-contoso-dev` (`env=dev`) and the cluster's node group
  `MC_rg-contoso-prod_aks-contoso-prod_westeurope`, all in westeurope.
- Resources: the five the other Azure domains know (`aks-contoso-prod`,
  `contosoacr`, `kv-contoso-prod`, `srch-contoso-prod` in `rg-contoso-prod`,
  `srch-contoso-dev` in `rg-contoso-dev`), plus `vnet-contoso-prod` and the
  network around it (facts/net.md): `nsg-contoso-aks`, `rt-contoso-aks`,
  `pe-kv-contoso-prod` and its NIC, `privatelink.vaultcore.azure.net` and its
  link `link-hub`, `contoso.example`, `agw-contoso-prod` and its public IP,
  and in the node group the load balancer `kubernetes` and its two public
  IPs, plus three user-assigned identities and the orders-db disk (below):
  twenty-two rows.
- `arm resource get aks-contoso-prod` reads the cluster at `2026-07-01`, the
  newest stable version Microsoft.ContainerService lists (a newer
  `2026-07-02-preview` is passed over): Kubernetes 1.30.4, Azure CNI overlay,
  workload identity and the OIDC issuer on, a system and a user pool of three
  nodes each in `vnet-contoso-prod/subnets/aks` (facts/aks.md).
- That subnet, `arm resource get …/virtualNetworks/vnet-contoso-prod/subnets/aks`,
  is `10.1.0.0/22`, read at `2024-05-01`, as `net subnet get` reads it.
- `arm query run 'resources | summarize count() by type'` answers one row per
  type (seventeen): two search services, three public IPs, three identities.
- Identities (`arm identity list`): the kubelet's `aks-contoso-prod-agentpool`
  (client `…c002`, principal `…c003`) and the Key Vault CSI add-on's
  `azurekeyvaultsecretsprovider-aks-contoso-prod` (`…c004`, `…c005`) in the
  node group, and `id-contoso-notifier` in rg-contoso-prod (client
  `00000000-0000-4000-8000-00000000b0f1`, principal `…b0f2`), whose one
  federated credential trusts the cluster's OIDC issuer (facts/aks.md) for
  `system:serviceaccount:web:notifier`, so `arm identity get` (by name or
  client id) names it `aks-contoso-prod/web/notifier`.
- Role assignments (`arm role list`): the group `…f001` is Contributor on
  the subscription; the kubelet AcrPull on `contosoacr`; the CSI add-on Key
  Vault Secrets User on `kv-contoso-prod` (what the worker's secrets mount
  through); the notifier only Monitoring Metrics Publisher on
  rg-contoso-prod. `--scope kv-contoso-prod` gives the first, the last
  (both inherited) and the add-on's.
- Trace 4, workload identity 403: the notifier's Key Vault Secrets User on
  `kv-contoso-prod` (assignment `…0a5e05`) was deleted at 2026-09-28T16:42:11Z
  by `dana@contoso.example`. `arm activity list` (the last day, `--group
  rg-contoso-prod` or `--resource kv-contoso-prod`) shows it, and `arm change
  list` (`--resource kv-contoso-prod` too) shows the Delete. For phase 5 and
  the traces step, the k8s side must add: in `prod/web`, a serviceaccount
  `notifier` annotated `azure.workload.identity/client-id:
  00000000-0000-4000-8000-00000000b0f1`, and a pod of a Deployment `notifier`
  (label `azure.workload.identity/use: "true"`, service account `notifier`)
  whose log says Key Vault answered 403 Forbidden for
  `https://kv-contoso-prod.vault.azure.net/secrets/…` (code Forbidden,
  `ForbiddenByRbac`, caller oid `…b0f2`).
- The activity log also holds `sam@contoso.example` failing to create the
  storage account `stcontosoreports` at 2026-09-29T09:14:31Z with
  RequestDisallowedByPolicy (assignment `deny-public-storage`), which
  `--status failed` finds.
- `arm policy list` (and `--group rg-contoso-prod`): one non-compliant state,
  `kv-contoso-prod` against the built-in "Azure Key Vault should disable
  public network access" (audit, assignment `kv-private-access`), consistent
  with trace 1's public vault address.
- `arm change list` (14 days) also has the user pool's count going 2 to 3 at
  2026-09-27T03:10:42Z, no changed_by (the autoscaler).
- The orders-db disk from trace 2 (`k8s pvc get`'s `pv.disk`,
  `…/MC_rg-contoso-prod_aks-contoso-prod_westeurope/providers/Microsoft.Compute/disks/pvc-7c3e9a1b-4d2f-4e8a-9b6c-1f0e2d3c4b5a`):
  `arm resource get` reads it at `2024-03-02` (Premium_LRS P6, 64 GiB, 240
  IOPS, zone 1, Attached, managedBy `aks-user-27182818-vmss/virtualMachines/1`).
  `arm metric list` on it lists the five Composite Disk metrics; with
  `--metric "Composite Disk Write Operations/sec" --interval 5m` (11:00 to
  12:00) writes run near 180/s, fall at 11:20 and are 0 from 11:25. A disk
  has no IOPS-consumed percentage (VERIFY(work)); the user pool's scale set
  `aks-user-27182818-vmss` answers "Data Disk IOPS Consumed Percentage", about
  77% to 11:15, then 0.
