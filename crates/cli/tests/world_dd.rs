//! What the contoso world answers of dd. Compiled only with the
//! `fixtures` feature.
#![cfg(feature = "fixtures")]

#[path = "common/world.rs"]
mod world;

use world::ok;

#[test]
fn the_world_answers_what_a_trial_asks_of_dd() {
    for args in [
        &["dd", "monitor", "list"][..],
        &["dd", "monitor", "get", "4712"],
        &["dd", "downtime", "list"],
        &[
            "dd",
            "log",
            "list",
            "--service",
            "worker",
            "--status",
            "error",
        ],
        &[
            "dd",
            "log",
            "list",
            "--pod",
            "prod/web/worker-5c4d3e9f1-q8zt1",
        ],
        &[
            "dd",
            "log",
            "list",
            "--pod",
            "prod/web/worker-5c4d3e9f1-q8zt1",
            "--status",
            "error",
            "--since",
            "1d",
        ],
        &["dd", "log-count", "list", "--status", "error"],
        &[
            "dd",
            "log-count",
            "list",
            "--status",
            "error",
            "--since",
            "1d",
        ],
        &["dd", "event", "list"],
        &["dd", "event", "list", "--since", "1d"],
        &[
            "dd",
            "metric",
            "get",
            "sum:trace.http.request.errors{service:api,env:prod}.as_count()",
            "--since",
            "1d",
        ],
        &["dd", "metric", "list", "trace"],
        &["dd", "service", "list"],
        &["dd", "service", "get", "api"],
        &[
            "dd",
            "service",
            "get",
            "api",
            "--since",
            "2026-09-28T21:30:00Z",
            "--until",
            "2026-09-28T22:30:00Z",
        ],
        &[
            "dd",
            "span",
            "list",
            "--service",
            "api",
            "--status",
            "error",
            "--since",
            "2026-09-28T21:30:00Z",
            "--until",
            "2026-09-28T22:30:00Z",
        ],
        &["dd", "incident", "list"],
        &["dd", "container", "list", "--namespace", "web"],
        &["dd", "host", "list"],
        &["dd", "dashboard", "list", "kubernetes"],
        &[
            "dd",
            "event",
            "list",
            "--since",
            "2026-09-28T21:00:00Z",
            "--until",
            "2026-09-28T23:00:00Z",
        ],
        &["doctor", "dd"],
    ] {
        ok(args);
    }
}
