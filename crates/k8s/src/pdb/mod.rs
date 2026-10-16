//! `k8s pdb`: PodDisruptionBudgets, read through `list` alone; one is read
//! whole through `k8s object get pdb`.

pub(crate) mod list;
