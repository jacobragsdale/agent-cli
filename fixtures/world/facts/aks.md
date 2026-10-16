# aks in the contoso world

What the recordings in `http/azure.json` say, consistent with every other
domain's facts. Keep a new recording consistent with these, and add a line
when you add a fact.

- Cluster `aks-contoso-prod` (Running).
- `aks cluster get aks-contoso-prod` reads it at `2026-07-01`: Kubernetes
  1.30.4, Standard tier, Azure CNI overlay with calico, a public API server
  open to `203.0.113.0/24`, the OIDC issuer and workload identity on, the
  kubelet identity `aks-contoso-prod-agentpool` in the node group, the Key
  Vault CSI add-on, auto-upgrade `patch`, and one maintenance window
  (`aksManagedAutoUpgradeSchedule`, Sundays 02:00 UTC for 4 hours).
- Two pools in `vnet-contoso-prod/subnets/aks`, both Standard_D4s_v5 on
  1.30.4 and node image `202509.10.0`: `system` (System, 3 nodes, tainted
  `CriticalAddonsOnly`) and `user` (User, autoscaled 1 to 3 and at 3, so
  `aks nodepool get aks-contoso-prod/user` notes its maximum: trace 3's
  pending pods).
- `aks upgrade list aks-contoso-prod`: the control plane can go to 1.30.9,
  1.31.5 or 1.32.1 (preview); both pools to 1.30.9 or 1.31.5, and node image
  `202510.03.0` is out for both.
- `aks command run aks-contoso-prod --yes -- kubectl get nodes` is accepted
  (202), then answers six Ready nodes, three per pool, exit code 0.
