//! The redis domain: keys, their values and expiry, and instance health on
//! Azure Cache for Redis, Azure Managed Redis and self-hosted Redis. Read
//! only: the door sends nothing but reads.

mod client;
mod config;
mod doctor;
mod instance;
mod key;
mod session;
mod slowlog;
#[cfg(test)]
mod testing;

use agent_cli_core::Domain;

pub const DOMAIN: Domain = Domain {
    name: "redis",
    summary: "Redis caches",
    commands: &[
        key::list::KEY_LIST,
        key::get::KEY_GET,
        key::diff::KEY_DIFF,
        instance::list::INSTANCE_LIST,
        instance::get::INSTANCE_GET,
        slowlog::list::SLOWLOG_LIST,
        client::list::CLIENT_LIST,
    ],
    synonyms: &[
        ("cache", &["redis"]),
        ("caches", &["redis"]),
        ("cached", &["redis"]),
        ("ttl", &["expires"]),
        ("expiry", &["expires"]),
        ("expire", &["expires"]),
        ("expiring", &["expires"]),
        ("slow", &["slowlog"]),
        ("connections", &["client"]),
        ("server", &["instance"]),
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
