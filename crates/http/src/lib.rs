//! The http domain: REST calls to any URL as curl makes them, OpenAPI specs
//! found, queried and compared, and the certificates servers present.
//!
//! What it adds to curl: a failure exits as one, output is bounded, the
//! sign-in comes from the environment and stays on its origin, writes are
//! guarded, and the proxy and certificates are core's. crates/http/AGENTS.md
//! is the card.

mod answer;
mod call;
mod certificate;
mod der;
mod doctor;
mod openapi;
mod operation;
mod request;
mod schema;
mod spec;
#[cfg(test)]
mod testing;
mod url;

use agent_cli_core::Domain;

pub const DOMAIN: Domain = Domain {
    name: "http",
    summary: "REST calls, OpenAPI specs, TLS",
    commands: &[
        request::get::REQUEST_GET,
        request::run::REQUEST_RUN,
        request::diff::REQUEST_DIFF,
        request::wait::REQUEST_WAIT,
        spec::get::SPEC_GET,
        spec::list::SPEC_LIST,
        spec::diff::SPEC_DIFF,
        operation::list::OPERATION_LIST,
        operation::get::OPERATION_GET,
        schema::list::SCHEMA_LIST,
        schema::get::SCHEMA_GET,
        certificate::get::CERTIFICATE_GET,
        certificate::list::CERTIFICATE_LIST,
    ],
    synonyms: &[
        ("curl", &["request", "get"]),
        ("rest", &["request"]),
        ("wget", &["request", "get"]),
        ("put", &["request", "run"]),
        ("patch", &["request", "run"]),
        ("openapi", &["spec"]),
        ("swagger", &["spec"]),
        ("oas", &["spec"]),
        ("endpoints", &["operation"]),
        ("model", &["schema"]),
        ("models", &["schema"]),
        ("dto", &["schema"]),
        ("tls", &["certificate"]),
        ("ssl", &["certificate"]),
        ("cert", &["certificate"]),
        ("x509", &["certificate"]),
        ("breaking", &["spec", "diff"]),
        ("healthz", &["request", "wait"]),
    ],
    status: doctor::status,
    doctor: doctor::doctor,
};

#[cfg(test)]
mod tests {
    use agent_cli_core::{check_layout, check_registry};

    use super::DOMAIN;

    #[test]
    fn the_registry_keeps_every_rule() {
        assert_eq!(check_registry(&[DOMAIN]), Vec::<String>::new());
        assert_eq!(check_layout(&[DOMAIN]), Vec::<String>::new());
    }
}
