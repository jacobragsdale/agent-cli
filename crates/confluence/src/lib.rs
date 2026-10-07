//! The confluence domain: Confluence Cloud spaces, pages read as bounded
//! Markdown and written from it, page trees, comments, attachments and
//! versions, over the v2 REST API with v1 where v2 has no call.
//!
//! Bodies are converted locally from storage (Markdown that names what it
//! simplified in `lossy`) and back, writes assert the version that was read,
//! and a replace never overwrites what Markdown cannot carry. Data Center is
//! not supported. Why each choice: crates/confluence/AGENTS.md.

mod attachment;
mod client;
mod comment;
mod compose;
mod config;
mod doctor;
mod ids;
mod markdown;
mod page;
mod space;
mod storage;
mod syntax;
#[cfg(test)]
mod testing;
mod tree;
mod version;

use agent_cli_core::Domain;

pub const DOMAIN: Domain = Domain {
    name: "confluence",
    summary: "Confluence Cloud",
    commands: &[
        space::list::SPACE_LIST,
        page::list::PAGE_LIST,
        page::get::PAGE_GET,
        page::create::PAGE_CREATE,
        page::update::PAGE_UPDATE,
        page::comment::PAGE_COMMENT,
        page::delete::PAGE_DELETE,
        tree::get::TREE_GET,
        comment::list::COMMENT_LIST,
        comment::update::COMMENT_UPDATE,
        attachment::list::ATTACHMENT_LIST,
        attachment::get::ATTACHMENT_GET,
        attachment::create::ATTACHMENT_CREATE,
        version::list::VERSION_LIST,
    ],
    synonyms: &[
        ("wiki", &["page"]),
        ("doc", &["page"]),
        ("docs", &["page"]),
        ("runbook", &["page"]),
        ("runbooks", &["page"]),
        ("article", &["page"]),
        ("blog", &["page"]),
        ("kb", &["page"]),
        ("children", &["tree"]),
        ("hierarchy", &["tree"]),
        ("revision", &["version"]),
        ("revisions", &["version"]),
        ("history", &["version"]),
    ],
    status: doctor::status,
    doctor: doctor::doctor,
};

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::assert_read_only_refuses;
    use agent_cli_core::{check_layout, check_registry};

    use super::*;

    #[test]
    fn the_registry_keeps_every_rule() {
        assert_eq!(check_registry(&[DOMAIN]), Vec::<String>::new());
        assert_eq!(check_layout(&[DOMAIN]), Vec::<String>::new());
        assert_eq!(DOMAIN.commands.len(), 14);
    }

    #[test]
    fn read_only_mode_refuses_every_change_before_any_request() {
        assert_read_only_refuses(&[DOMAIN]);
    }
}
