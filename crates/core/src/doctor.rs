//! `agent-cli doctor`: core's row (the build, the config, whether a newer
//! release is out), then each domain's live checks, recording the domains a
//! 403 shuts out so search and the overview can leave them out.

use std::io::Write;
use std::time::Duration;

use anyhow::Result;
use serde::Serialize;
use serde_json::{Value, json};

use crate::config::Config;
use crate::ctx::Ctx;
use crate::dispatch::unknown;
use crate::error::{Exit, Failure};
use crate::hidden;
use crate::http::Request;
use crate::output;
use crate::registry::Domain;
use crate::secret::redact_value;

/// The most the release check may take: doctor must not wait on a server
/// that is not there.
const RELEASE_WAIT: Duration = Duration::from_secs(3);
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// One line of `agent-cli doctor`.
#[derive(Clone, Debug, Serialize)]
pub struct Check {
    pub check: String,
    pub ok: bool,
    pub detail: String,
    pub hint: Option<String>,
    /// The service refused this login (a 403), as opposed to being down or
    /// misconfigured: a domain whose only reads are denied is left out of
    /// search and the overview. Core also marks a failed check whose detail
    /// says `answered 403`.
    pub denied: bool,
}

impl Check {
    #[must_use]
    pub fn ok(check: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            check: check.into(),
            ok: true,
            detail: detail.into(),
            hint: None,
            denied: false,
        }
    }

    #[must_use]
    pub fn failed(
        check: impl Into<String>,
        detail: impl Into<String>,
        hint: impl Into<String>,
    ) -> Self {
        Self {
            check: check.into(),
            ok: false,
            detail: detail.into(),
            hint: Some(hint.into()),
            denied: false,
        }
    }

    /// Marks a failed check as refused for lack of a role; say which role
    /// in its detail or hint when the domain knows it.
    #[must_use]
    pub fn denied(mut self) -> Self {
        self.denied = !self.ok;
        self
    }
}

pub(crate) fn builtin(
    domains: &[Domain],
    args: &[String],
    ctx: &Ctx,
    out: &mut dyn Write,
    err: &mut dyn Write,
    tty: bool,
) -> Result<Exit> {
    if ctx.globals().help {
        writeln!(
            out,
            "usage: agent-cli doctor [domain]   live checks as JSON [{{domain,check,ok,detail,hint}}]; exit 1 if any failed"
        )?;
        return Ok(Exit::Ok);
    }
    let selected: Vec<&Domain> = match args {
        [] => domains.iter().collect(),
        [name] => match domains.iter().find(|domain| domain.name == name) {
            Some(domain) => vec![domain],
            None => {
                let names = domains.iter().map(|domain| domain.name);
                return Err(unknown("domain", name, "", names, domains, args, None));
            }
        },
        _ => return Err(Failure::usage("usage: agent-cli doctor [domain]").into()),
    };
    let mut rows = vec![core_row(ctx)];
    let named = !args.is_empty();
    let mut checked = Vec::new();
    for domain in selected {
        let mut checks = (domain.doctor)(ctx);
        // A domain's doctor checks nothing until its section is there. Not
        // set up fails only a doctor that was asked for that domain.
        if checks.is_empty() {
            rows.push(json!({
                "domain": domain.name,
                "check": "config",
                "ok": !named,
                "detail": "not set up",
                "hint": format!("agent-cli config example {}", domain.name),
            }));
        }
        for check in &mut checks {
            check.denied |= !check.ok && check.detail.contains(" answered 403");
        }
        checked.push((domain.name, hidden::denied_detail(&checks)));
        for check in checks {
            let mut row = json!({
                "domain": domain.name,
                "check": check.check,
                "ok": check.ok,
                "detail": check.detail,
                "hint": check.hint,
            });
            if check.denied {
                row["denied"] = json!(true);
            }
            rows.push(row);
        }
    }
    if let Some(dir) = ctx.state_dir.as_deref() {
        hidden::record(dir, &checked);
    }
    let failed = rows.iter().any(|row| row["ok"] == false);
    output::emit(
        redact_value(Value::Array(rows)),
        ctx.globals(),
        false,
        out,
        err,
        tty,
    )?;
    Ok(if failed { Exit::Failed } else { Exit::Ok })
}

/// The config file's row, naming the build too: is the binary current?
fn core_row(ctx: &Ctx) -> Value {
    let config = ctx.config();
    let mut row = config_row(config);
    let release = match config.core("releases") {
        None => String::new(),
        Some(releases) => match latest(ctx, &releases) {
            Ok(latest) if newer(&latest, VERSION) => {
                format!(" ({latest} is out; rerun install.sh)")
            }
            Ok(_) => " (the latest)".to_owned(),
            Err(error) => format!(" (the latest release is unknown: {error:#})"),
        },
    };
    row["detail"] = json!(format!(
        "agent-cli {VERSION}{release} \u{b7} {}",
        row["detail"].as_str().unwrap_or_default()
    ));
    row
}

fn config_row(config: &Config) -> Value {
    let path = config.path().display().to_string();
    match config.problem() {
        Some(problem) => json!({
            "domain": "core", "check": "config", "ok": false, "detail": problem,
            "hint": format!("fix {path}; `agent-cli config example` shows every key"),
        }),
        None if config.found() => {
            json!({"domain": "core", "check": "config", "ok": true, "detail": path})
        }
        None => json!({
            "domain": "core", "check": "config", "ok": true,
            "detail": format!("{path} does not exist, so every section is empty"),
        }),
    }
}

/// `{releases}/latest/VERSION`, anonymous, within [`RELEASE_WAIT`]: one
/// attempt, no retry, since it is only ever a detail.
fn latest(ctx: &Ctx, releases: &str) -> Result<String> {
    let url = format!("{}/latest/VERSION", releases.trim_end_matches('/'));
    let wait = ctx.remaining()?.min(RELEASE_WAIT);
    let response = ctx
        .meter
        .sent(ctx.transport().send(&Request::get(&url), None, wait))?;
    if !(200..300).contains(&response.status) {
        anyhow::bail!("{url} answered {}", response.status);
    }
    let version = response.body.trim().trim_start_matches('v');
    if version.is_empty() || parts(version).is_none() {
        anyhow::bail!("{url} answered something other than a version");
    }
    Ok(version.to_owned())
}

fn parts(version: &str) -> Option<Vec<u64>> {
    version.split('.').map(|part| part.parse().ok()).collect()
}

fn newer(latest: &str, running: &str) -> bool {
    match (parts(latest), parts(running)) {
        (Some(latest), Some(running)) => latest > running,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctx::Setup;
    use crate::testing::{Answer, FakeTransport, ctx};

    #[test]
    fn a_version_is_newer_by_its_numbers_not_its_text() {
        assert!(newer("0.10.0", "0.9.3"));
        assert!(!newer("0.2.0", "0.2.0"));
        assert!(!newer("garbage", "0.2.0"));
    }

    #[test]
    fn the_core_row_says_when_a_release_is_out_and_never_fails_for_it() {
        let config = "[core]\nreleases = \"https://keeper.contoso.example/api/v1/repositories/agent-cli/download/\"\n";
        let transport = FakeTransport::answering([Answer::ok("99.0.0\n")]);
        let row = core_row(&ctx(Setup::fake(transport.clone()).with_config(config)));
        assert_eq!(row["ok"], true);
        assert!(
            row["detail"]
                .as_str()
                .unwrap()
                .contains("(99.0.0 is out; rerun install.sh)"),
            "{row}"
        );
        assert_eq!(
            transport.sent()[0].url,
            "https://keeper.contoso.example/api/v1/repositories/agent-cli/download/latest/VERSION"
        );
        assert_eq!(transport.sent()[0].authorization, None);

        let down = FakeTransport::answering([Answer::status(502, "")]);
        let row = core_row(&ctx(Setup::fake(down).with_config(config)));
        assert_eq!(row["ok"], true);
        assert!(
            row["detail"]
                .as_str()
                .unwrap()
                .contains("the latest release is unknown"),
            "{row}"
        );

        let none = FakeTransport::default();
        let row = core_row(&ctx(Setup::fake(none.clone())));
        assert!(none.sent().is_empty(), "no releases key, no request: {row}");
    }
}
