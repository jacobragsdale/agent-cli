//! `aks`: AKS clusters. `cluster list` reads the Resource Graph inventory;
//! `cluster connect` is az-tui's `setup` for one cluster, without the config
//! file writing: `az aks get-credentials` then `kubelogin convert-kubeconfig`,
//! and a ready-to-paste `[[k8s.scope]]` block for the k8s domain.

pub(crate) mod cluster;
