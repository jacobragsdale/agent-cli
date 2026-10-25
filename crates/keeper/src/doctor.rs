//! The overview's line for keeper, and `agent-cli doctor keeper`.

use agent_cli_core::{Check, Config, Ctx, status_of};

use crate::client::{Keeper, Section, text};

/// `keeper keeper.contoso.example`, from config alone: the server's host.
pub(crate) fn status(config: &Config) -> String {
    if !config.has_section("keeper") {
        return "keeper not set up".to_owned();
    }
    match config
        .section::<Section>("keeper")
        .and_then(|section| section.base())
    {
        Ok((_, host)) => format!("keeper {host}"),
        Err(_) => "keeper config broken".to_owned(),
    }
}

/// The address, where the token comes from (or that there is none), the
/// server's version from `/health`, then one read as the caller: who the
/// token signs in as, or that anonymous reads are allowed.
pub(crate) fn doctor(ctx: &Ctx) -> Vec<Check> {
    if !ctx.config().has_section("keeper") {
        return Vec::new();
    }
    let keeper = match Keeper::load(ctx) {
        Ok(keeper) => keeper,
        Err(error) => {
            return vec![Check::failed(
                "config",
                format!("{error:#}"),
                "fix [keeper]; `agent-cli config example keeper` shows every key",
            )];
        }
    };
    let mut checks = vec![
        Check::ok("config", format!("url {}", keeper.base)),
        Check::ok("credential", keeper.source()),
    ];
    let health = format!("{}/health", keeper.base);
    match keeper.get(ctx, &health) {
        Ok(health) => checks.push(Check::ok(
            "server",
            format!(
                "Artifact Keeper {}, {}",
                text(&health["version"]).unwrap_or_else(|| "(no version)".to_owned()),
                text(&health["status"]).unwrap_or_else(|| "status unknown".to_owned())
            ),
        )),
        Err(error) => {
            checks.push(Check::failed(
                "server",
                format!("{error:#}"),
                "check url (the address the browser shows) and that this machine reaches it",
            ));
            return checks;
        }
    }
    let (url, what) = if keeper.signed_in() {
        (keeper.api("/auth/me", &[]), "sign-in")
    } else {
        (
            keeper.api("/repositories", &[("per_page", "1".to_owned())]),
            "anonymous read",
        )
    };
    checks.push(match keeper.get(ctx, &url) {
        Ok(me) if keeper.signed_in() => Check::ok(
            "access",
            format!(
                "signed in as {}",
                text(&me["username"]).unwrap_or_else(|| "a known user".to_owned())
            ),
        ),
        Ok(listed) => Check::ok(
            "access",
            format!(
                "anonymous reads allowed: {} public repositories",
                listed["pagination"]["total"].as_u64().unwrap_or_default()
            ),
        ),
        Err(error) if status_of(&error) == Some(403) => Check::failed(
            "access",
            format!("{error:#}"),
            "ask a Keeper admin for read access to the repositories you need",
        )
        .denied(),
        Err(error) => Check::failed(
            "access",
            format!("{what}: {error:#}"),
            match keeper.signed_in() {
                true => format!(
                    "the token was refused: make one at {}/access-tokens",
                    keeper.base
                ),
                false => format!(
                    "the server allows no anonymous reads: make an API token at {}/access-tokens and set token_env under [keeper]",
                    keeper.base
                ),
            },
        ),
    });
    checks
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use super::*;
    use crate::testing::{ANONYMOUS, API, BASE, CONFIG, anonymous, keeper_run, page, urls};

    #[test]
    fn the_overview_names_the_server() {
        let config = |toml: &str| Config::parse("c.toml", Some(toml), Vec::new());
        assert_eq!(status(&config(CONFIG)), "keeper keeper.contoso.example");
        assert_eq!(status(&config("")), "keeper not set up");
        assert_eq!(
            status(&config(
                "[keeper]\nurl = \"http://keeper.contoso.example\"\n"
            )),
            "keeper config broken"
        );
        assert_eq!(status(&config(ANONYMOUS)), "keeper keeper.contoso.example");
    }

    fn checks(outcome: &agent_cli_core::testing::Outcome) -> Vec<(String, bool, String)> {
        outcome
            .json()
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["domain"] == "keeper")
            .map(|row| {
                (
                    row["check"].as_str().unwrap().to_owned(),
                    row["ok"].as_bool().unwrap(),
                    row["detail"].as_str().unwrap_or_default().to_owned(),
                )
            })
            .collect()
    }

    fn health() -> Answer {
        Answer::json(
            &json!({"status": "healthy", "version": "1.11.7", "demo_mode": false,
            "checks": {"database": {"status": "healthy"}, "storage": {"status": "healthy"}}}),
        )
    }

    #[test]
    fn doctor_names_the_version_and_who_the_token_signs_in_as() {
        let (outcome, transport) = keeper_run(
            &["doctor", "keeper"],
            vec![
                health(),
                Answer::json(&json!({"id": "u1", "username": "svc-agent", "is_admin": false})),
            ],
        );
        assert_eq!(
            urls(&transport),
            [format!("{BASE}/health"), format!("{API}/auth/me")]
        );
        let checks = checks(&outcome);
        assert_eq!(
            checks,
            [
                ("config".into(), true, format!("url {BASE}")),
                (
                    "credential".into(),
                    true,
                    format!("token_env KEEPER_TEST_TOKEN (Bearer) to {BASE}/")
                ),
                (
                    "server".into(),
                    true,
                    "Artifact Keeper 1.11.7, healthy".into()
                ),
                ("access".into(), true, "signed in as svc-agent".into()),
            ]
        );
    }

    #[test]
    fn doctor_without_a_token_reads_anonymously_and_marks_a_403_denied() {
        let (outcome, transport) =
            anonymous(&["doctor", "keeper"], vec![health(), page(vec![], 3)]);
        assert_eq!(
            urls(&transport)[1],
            format!("{API}/repositories?per_page=1")
        );
        assert_eq!(transport.sent()[1].authorization, None);
        let checks = checks(&outcome);
        assert_eq!(checks[1].2, "anonymous: public repositories only");
        assert_eq!(
            checks[3].2,
            "anonymous reads allowed: 3 public repositories"
        );

        let (outcome, _) = keeper_run(
            &["doctor", "keeper"],
            vec![
                health(),
                Answer::status(403, r#"{"code":"FORBIDDEN","message":"Forbidden"}"#),
            ],
        );
        let rows = outcome.json();
        let access = rows
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["domain"] == "keeper" && row["check"] == "access")
            .unwrap();
        assert_eq!(access["ok"], false);
        assert_eq!(access["denied"], true, "{access}");
    }
}
