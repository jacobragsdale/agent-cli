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
  `srch-contoso-dev` in `rg-contoso-dev`), plus `vnet-contoso-prod`.
- `arm resource get aks-contoso-prod` reads the cluster at `2025-01-01`, the
  newest stable version Microsoft.ContainerService lists (a newer
  `2025-02-02-preview` is passed over): Kubernetes 1.30.4, Azure CNI overlay,
  workload identity and the OIDC issuer on, one system pool of three nodes in
  `vnet-contoso-prod/subnets/aks`.
- That subnet, `arm resource get …/virtualNetworks/vnet-contoso-prod/subnets/aks`,
  is `10.1.0.0/22`, read at `2024-05-01`.
- `arm query run 'resources | summarize count() by type'` answers one row per
  type, two search services.
