//! The ado domain: work items, pull requests, pipelines, runs and approvals,
//! live from Azure DevOps. Ported from ticket-tui without its SQLite cache:
//! every read asks Azure DevOps (WIQL and REST), so an answer is never older
//! than the command.

mod approval;
mod client;
mod diff;
mod doctor;
mod file;
mod ids;
mod markdown;
mod pipeline;
mod pr;
mod repo;
mod run;
mod team;
#[cfg(test)]
mod testing;
mod thread;
mod work_items;
mod workitem;

use agent_cli_core::Domain;

pub const DOMAIN: Domain = Domain {
    name: "ado",
    summary: "Azure DevOps",
    commands: &[
        workitem::list::WORKITEM_LIST,
        workitem::get::WORKITEM_GET,
        workitem::create::WORKITEM_CREATE,
        workitem::update::WORKITEM_UPDATE,
        workitem::comment::WORKITEM_COMMENT,
        workitem::link::WORKITEM_LINK,
        team::list::TEAM_LIST,
        repo::list::REPO_LIST,
        repo::get::REPO_GET,
        diff::get::DIFF_GET,
        file::get::FILE_GET,
        file::list::FILE_LIST,
        pr::list::PR_LIST,
        pr::get::PR_GET,
        pr::create::PR_CREATE,
        pr::vote::PR_VOTE,
        pr::update::PR_UPDATE,
        pr::link::PR_LINK,
        pr::comment::PR_COMMENT,
        pr::complete::PR_COMPLETE,
        pr::abandon::PR_ABANDON,
        thread::list::THREAD_LIST,
        thread::comment::THREAD_COMMENT,
        thread::update::THREAD_UPDATE,
        pipeline::list::PIPELINE_LIST,
        run::list::RUN_LIST,
        run::get::RUN_GET,
        run::logs::RUN_LOGS,
        run::create::RUN_CREATE,
        run::wait::RUN_WAIT,
        run::cancel::RUN_CANCEL,
        run::retry::RUN_RETRY,
        approval::list::APPROVAL_LIST,
        approval::approve::APPROVAL_APPROVE,
        approval::reject::APPROVAL_REJECT,
    ],
    synonyms: &[
        ("ticket", &["workitem"]),
        ("tickets", &["workitem"]),
        ("bug", &["workitem"]),
        ("bugs", &["workitem"]),
        ("story", &["workitem"]),
        ("stories", &["workitem"]),
        ("user story", &["workitem"]),
        ("task", &["workitem"]),
        ("tasks", &["workitem"]),
        ("issue", &["workitem"]),
        ("issues", &["workitem"]),
        ("backlog", &["workitem"]),
        ("work item", &["workitem"]),
        ("work items", &["workitem"]),
        ("review comment", &["thread"]),
        ("review comments", &["thread"]),
        ("comment thread", &["thread"]),
        ("changes", &["diff"]),
        ("changed files", &["diff"]),
        ("compare", &["diff"]),
        ("prs", &["pr"]),
        ("pull request", &["pr"]),
        ("pull requests", &["pr"]),
        ("merge request", &["pr"]),
        ("review", &["pr"]),
        ("build", &["run", "pipeline"]),
        ("builds", &["run", "pipeline"]),
        ("ci", &["run", "pipeline"]),
        ("sprint", &["iteration"]),
        ("gate", &["approval"]),
        ("repository", &["repo"]),
        ("repositories", &["repo"]),
        ("azure devops", &["ado"]),
        ("devops", &["ado"]),
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
        assert_eq!(DOMAIN.commands.len(), 35);
    }

    #[test]
    fn read_only_mode_refuses_every_change_before_any_request() {
        agent_cli_core::testing::assert_read_only_refuses(&[DOMAIN]);
    }
}
