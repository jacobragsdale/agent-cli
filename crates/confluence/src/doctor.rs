//! The overview's line for confluence, and `agent-cli doctor confluence`.

use agent_cli_core::{Check, Config, Ctx};

use crate::client::{Confluence, TOKEN_PAGE, text};
use crate::config::Section;

/// `confluence contoso`, from config alone: the site's name.
pub(crate) fn status(config: &Config) -> String {
    if !config.has_section("confluence") {
        return "confluence not set up".to_owned();
    }
    match config
        .section::<Section>("confluence")
        .and_then(|section| section.site())
    {
        Ok(site) => format!("confluence {}", site.name()),
        Err(_) => "confluence config broken".to_owned(),
    }
}

/// The site, where the token comes from (never its value), then who the
/// site says is calling. A bad token on a site that lets anyone read is
/// answered as the anonymous user, never 401, so anonymous is a failure.
pub(crate) fn doctor(ctx: &Ctx) -> Vec<Check> {
    if !ctx.config().has_section("confluence") {
        return Vec::new();
    }
    let confluence = match Confluence::load(ctx) {
        Ok(confluence) => confluence,
        Err(error) => {
            return vec![Check::failed(
                "config",
                format!("{error:#}"),
                "fix [confluence]; `agent-cli config example confluence` shows every key",
            )];
        }
    };
    let mut checks = vec![Check::ok("config", format!("site {}", confluence.site.web))];
    let Some(source) = confluence.source() else {
        checks.push(Check::failed(
            "credential",
            "none found",
            format!("make an API token at {TOKEN_PAGE} and set token_env or token_cmd under [confluence]"),
        ));
        return checks;
    };
    checks.push(Check::ok("credential", source));
    let url = confluence.v1("/user/current", &[]);
    checks.push(match confluence.get(ctx, &url) {
        Ok(user) if user["type"] == "known" => Check::ok(
            "connection",
            format!(
                "signed in as {}",
                text(&user["displayName"])
                    .or_else(|| text(&user["publicName"]))
                    .unwrap_or_else(|| "a known user".to_owned())
            ),
        ),
        Ok(user) => Check::failed(
            "connection",
            format!(
                "the site answered as the {} user: the token was ignored",
                text(&user["type"]).unwrap_or_else(|| "anonymous".to_owned())
            ),
            format!(
                "the token has expired, or belongs to another account than [confluence] email; make one at {TOKEN_PAGE}"
            ),
        ),
        Err(error) => Check::failed(
            "connection",
            format!("{error:#}"),
            format!("check url, email and the token (make one at {TOKEN_PAGE})"),
        ),
    });
    checks
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use super::*;
    use crate::testing::{CONFIG, confluence, urls};

    #[test]
    fn the_overview_names_the_site() {
        let config = |toml: &str| Config::parse("c.toml", Some(toml), Vec::new());
        assert_eq!(status(&config(CONFIG)), "confluence contoso");
        assert_eq!(status(&config("")), "confluence not set up");
        assert_eq!(
            status(&config(
                "[confluence]\nurl = \"https://wiki.contoso.example\"\n"
            )),
            "confluence config broken"
        );
    }

    fn checks(user: serde_json::Value) -> Vec<(String, bool, String)> {
        let (outcome, transport) = confluence(&["doctor", "confluence"], vec![Answer::json(&user)]);
        assert_eq!(
            urls(&transport),
            ["https://contoso.atlassian.net/wiki/rest/api/user/current"]
        );
        outcome
            .json()
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["domain"] == "confluence")
            .map(|row| {
                (
                    row["check"].as_str().unwrap().to_owned(),
                    row["ok"].as_bool().unwrap(),
                    row["detail"].as_str().unwrap_or_default().to_owned(),
                )
            })
            .collect()
    }

    #[test]
    fn doctor_needs_a_known_user_and_never_prints_the_token() {
        let known =
            checks(json!({"type": "known", "accountId": "557058:jane", "displayName": "Jane Doe"}));
        assert_eq!(
            known
                .iter()
                .map(|(check, ok, _)| (check.as_str(), *ok))
                .collect::<Vec<_>>(),
            [("config", true), ("credential", true), ("connection", true)]
        );
        assert!(
            known[1]
                .2
                .contains("token_env CONFLUENCE_TEST_TOKEN as jane@contoso.com"),
            "{known:?}"
        );
        assert_eq!(known[2].2, "signed in as Jane Doe");
        let anonymous = checks(json!({"type": "anonymous", "displayName": "Anonymous"}));
        assert!(
            !anonymous[2].1 && anonymous[2].2.contains("the token was ignored"),
            "{anonymous:?}"
        );
    }
}
