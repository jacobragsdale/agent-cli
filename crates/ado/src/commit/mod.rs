//! A repository's history. Azure DevOps has no blame, so "which change
//! touched this line" is the commits on its file, each with its pull
//! request and the diff to read.

pub(crate) mod list;
