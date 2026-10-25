//! The overview's line for confluence, and `agent-cli doctor confluence`.

use agent_cli_core::{Check, Config, Ctx, status_of};

use crate::client::{Confluence, person, text};
use crate::config::Section;

/// `confluence wiki.contoso.example`, from config alone: the site's host.
pub(crate) fn status(config: &Config) -> String {
    if !config.has_section("confluence") {
        return "confluence not set up".to_owned();
    }
    match config
        .section::<Section>("confluence")
        .and_then(|section| section.site())
    {
        Ok(site) => format!("confluence {}", site.host),
        Err(_) => "confluence config broken".to_owned(),
    }
}

/// The site, where the token comes from (never its value), then who the
/// site says is calling. A site that lets anyone read may answer a call
/// it did not sign in as the anonymous user, so anonymous is a failure.
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
    let mut checks = vec![Check::ok(
        "config",
        format!("site {}", confluence.site.base),
    )];
    let tokens = confluence.token_page();
    let Some(source) = confluence.source() else {
        checks.push(Check::failed(
            "credential",
            "none found",
            format!("make a personal access token at {tokens} and set token_env or token_cmd under [confluence]"),
        ));
        return checks;
    };
    checks.push(Check::ok("credential", source));
    let url = confluence.api("/user/current", &[]);
    checks.push(match confluence.get(ctx, &url) {
        Ok(user) if user["type"] == "known" => Check::ok(
            "connection",
            match (person(&user), text(&user["username"])) {
                (Some(name), Some(username)) if name != username => {
                    format!("signed in as {name} ({username})")
                }
                (name, username) => format!(
                    "signed in as {}",
                    name.or(username)
                        .unwrap_or_else(|| "a known user".to_owned())
                ),
            },
        ),
        Ok(user) => Check::failed(
            "connection",
            format!(
                "the site answered as the {} user: the token was not taken",
                text(&user["type"]).unwrap_or_else(|| "anonymous".to_owned())
            ),
            format!("make a token at {tokens}; an admin may also have turned personal access tokens off"),
        ),
        Err(error) if status_of(&error) == Some(401) => Check::failed(
            "connection",
            format!("{error:#}"),
            format!("the token has expired or was revoked; make one at {tokens}"),
        ),
        Err(error) => Check::failed(
            "connection",
            format!("{error:#}"),
            "check url (the address the browser shows, context path included) and that this machine reaches it",
        ),
    });
    checks
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use super::*;
    use crate::testing::{API, CONFIG, JANE, confluence, urls, user};

    #[test]
    fn the_overview_names_the_site() {
        let config = |toml: &str| Config::parse("c.toml", Some(toml), Vec::new());
        assert_eq!(status(&config(CONFIG)), "confluence wiki.contoso.example");
        assert_eq!(status(&config("")), "confluence not set up");
        assert_eq!(
            status(&config(
                "[confluence]\nurl = \"https://contoso.atlassian.net/wiki\"\n"
            )),
            "confluence config broken"
        );
    }

    fn checks(answer: Answer) -> Vec<(String, bool, String)> {
        let (outcome, transport) = confluence(&["doctor", "confluence"], vec![answer]);
        assert_eq!(urls(&transport), [format!("{API}/user/current")]);
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
        let known = checks(Answer::json(&user(JANE, "jane", "Jane Doe")));
        assert_eq!(
            known
                .iter()
                .map(|(check, ok, _)| (check.as_str(), *ok))
                .collect::<Vec<_>>(),
            [("config", true), ("credential", true), ("connection", true)]
        );
        assert_eq!(
            known[1].2,
            "token_env CONFLUENCE_TEST_TOKEN (Bearer) to https://wiki.contoso.example/confluence/"
        );
        assert_eq!(known[2].2, "signed in as Jane Doe (jane)");
        let anonymous = checks(Answer::json(
            &json!({"type": "anonymous", "displayName": "Anonymous"}),
        ));
        assert!(
            !anonymous[2].1 && anonymous[2].2.contains("the token was not taken"),
            "{anonymous:?}"
        );
    }
}
