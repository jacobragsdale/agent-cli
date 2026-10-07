//! The controlm domain: BMC Control-M's job runs (status, why they wait,
//! their output) and job definitions, and the two changes an operator makes
//! most, rerun and order, over an on-prem Enterprise Manager's Automation
//! API.
//!
//! Written from BMC's spec before meeting a real Enterprise Manager: every
//! `VERIFY(work)` names what to check against one, and docs/plans/controlm.md
//! is the order to check them in.

mod client;
mod definition;
mod doctor;
mod job;
#[cfg(test)]
mod testing;

use agent_cli_core::Domain;

pub const DOMAIN: Domain = Domain {
    name: "controlm",
    summary: "BMC Control-M",
    commands: &[
        job::list::JOB_LIST,
        job::get::JOB_GET,
        job::logs::JOB_LOGS,
        job::retry::JOB_RETRY,
        job::run::JOB_RUN,
        definition::list::DEFINITION_LIST,
        definition::get::DEFINITION_GET,
    ],
    synonyms: &[
        ("ctm", &["controlm"]),
        ("control", &["controlm"]),
        ("batch", &["job"]),
        ("sysout", &["job"]),
        ("rerun", &["job"]),
        ("folder", &["definition"]),
        ("folders", &["definition"]),
        ("defines", &["definition"]),
        ("planning", &["definition"]),
        ("schedule", &["definition"]),
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
        assert_eq!(DOMAIN.commands.len(), 7);
    }

    #[test]
    fn read_only_mode_refuses_every_change_before_any_request() {
        assert_read_only_refuses(&[DOMAIN]);
    }
}
