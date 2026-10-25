//! The keeper domain: Artifact Keeper, the self-hosted artifact registry, over
//! its REST API (`/api/v1`): repositories, the packages and versions they
//! hold, the files of a version, and what a PyPI proxy has cached. Read only.
//!
//! Anonymous by default (public repositories), with an optional API token for
//! private ones. Why each choice: crates/keeper/AGENTS.md.

mod client;
mod doctor;
mod file;
mod ids;
mod package;
mod repo;
#[cfg(test)]
mod testing;
mod version;

use agent_cli_core::Domain;

pub const DOMAIN: Domain = Domain {
    name: "keeper",
    summary: "Artifact Keeper",
    commands: &[
        repo::list::REPO_LIST,
        repo::get::REPO_GET,
        package::list::PACKAGE_LIST,
        package::get::PACKAGE_GET,
        version::get::VERSION_GET,
        file::list::FILE_LIST,
        file::get::FILE_GET,
    ],
    synonyms: &[
        ("artifactkeeper", &["keeper"]),
        ("artifactory", &["keeper"]),
        ("nexus", &["keeper"]),
        ("repository", &["repo"]),
        ("repositories", &["repo"]),
        ("feed", &["repo"]),
        ("artifacts", &["file"]),
        ("pip", &["pypi", "package"]),
        ("wheel", &["package", "file"]),
        ("whl", &["package", "file"]),
        ("sdist", &["package", "file"]),
        ("library", &["package"]),
        ("proxy", &["remote", "cached"]),
        ("mirror", &["remote", "cached"]),
        ("release", &["version"]),
        ("releases", &["version"]),
        ("published", &["created", "published_by"]),
        ("uploaded", &["created", "published_by"]),
        ("checksum", &["sha256"]),
        ("hash", &["sha256"]),
        ("download", &["file", "output"]),
    ],
    status: doctor::status,
    doctor: doctor::doctor,
};

#[cfg(test)]
mod tests {
    use agent_cli_core::{check_layout, check_registry};

    use super::*;

    #[test]
    fn the_registry_keeps_every_rule() {
        assert_eq!(check_registry(&[DOMAIN]), Vec::<String>::new());
        assert_eq!(check_layout(&[DOMAIN]), Vec::<String>::new());
        assert_eq!(DOMAIN.commands.len(), 7);
    }
}
