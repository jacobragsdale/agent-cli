//! The overview's line for dd, and `agent-cli doctor dd`.

use agent_cli_core::{Check, Config, Ctx, Method};

use crate::client::{Dd, Section, site};

/// `dd eu`, from config alone: the site's short name.
pub(crate) fn status(config: &Config) -> String {
    if !config.has_section("datadog") && !config.has_section("dd") {
        return "dd not set up".to_owned();
    }
    match Section::load(config).and_then(|section| site(section.site.as_deref())) {
        Ok(site) => format!("dd {}", site.label),
        Err(_) => "dd config broken".to_owned(),
    }
}

/// The site and where the credential comes from (never its value), then one
/// live call: `validate` for a key pair, a one-row monitor search for a
/// token. pup, when it is on PATH, covers what dd does not.
pub(crate) fn doctor(ctx: &Ctx) -> Vec<Check> {
    let configured = ctx.config().has_section("datadog")
        || ctx.config().has_section("dd")
        || ctx.env("DD_ACCESS_TOKEN").is_some()
        || ctx.env("DD_API_KEY").is_some();
    if !configured {
        return Vec::new();
    }
    let dd = match Dd::load(ctx) {
        Ok(dd) => dd,
        Err(error) => {
            return vec![Check::failed(
                "config",
                format!("{error:#}"),
                "fix [datadog]; config.example.toml shows every key",
            )];
        }
    };
    let mut checks = vec![Check::ok(
        "config",
        format!("site {} ({})", dd.site.name, dd.site.app),
    )];
    let Some(source) = dd.source() else {
        checks.push(Check::failed(
            "credential",
            "none found",
            "export DD_ACCESS_TOKEN, or set token_cmd = \"pup auth token\" under [datadog]",
        ));
        return checks;
    };
    checks.push(Check::ok("credential", source));
    let (path, query) = if dd.uses_keys() {
        ("/api/v1/validate", Vec::new())
    } else {
        ("/api/v1/monitor/search", vec![("per_page", "1".to_owned())])
    };
    checks.push(
        match dd.send(ctx, None, Method::Get, dd.url(path, &query), None) {
            Ok(_) => Check::ok("connection", format!("api.{} answers", dd.site.name)),
            Err(error) => Check::failed(
                "connection",
                format!("{error:#}"),
                "a 403 names the scope the credential lacks: grant it, or use a token that has it",
            ),
        },
    );
    let mut pup = std::process::Command::new("pup");
    pup.arg("--version");
    if let Ok(output) = ctx.read(pup)
        && output.status.success()
    {
        checks.push(Check::ok(
            "pup",
            format!(
                "{} covers what dd does not; run it with PUP_READ_ONLY=1",
                output.stdout.trim()
            ),
        ));
    }
    checks
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;

    use serde_json::json;

    use super::*;

    use crate::testing::{CONFIG, dd};

    #[test]
    fn the_overview_names_the_site() {
        let config = |toml: &str| Config::parse("c.toml", Some(toml), Vec::new());
        assert_eq!(status(&config(CONFIG)), "dd eu");
        assert_eq!(status(&config("[dd]\n")), "dd us1");
        assert_eq!(status(&config("")), "dd not set up");
        assert_eq!(
            status(&config("[datadog]\nsite = \"dd.contoso.example\"\n")),
            "dd config broken"
        );
    }

    #[test]
    fn doctor_names_the_site_and_the_credential_source_never_its_value() {
        let (outcome, transport) = dd(
            &["doctor", "dd"],
            vec![Answer::json(
                &json!({"monitors": [], "metadata": {"total_count": 0}}),
            )],
        );
        let rows = outcome.json();
        let checks: Vec<(&str, bool)> = rows
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["domain"] == "dd")
            .map(|row| (row["check"].as_str().unwrap(), row["ok"].as_bool().unwrap()))
            .collect();
        assert_eq!(
            &checks[..3],
            [("config", true), ("credential", true), ("connection", true)]
        );
        assert!(
            outcome.stdout.contains("token_env DD_TEST_TOKEN"),
            "{}",
            outcome.stdout
        );
        assert!(
            transport.sent()[0]
                .url
                .contains("/api/v1/monitor/search?per_page=1")
        );
    }
}
