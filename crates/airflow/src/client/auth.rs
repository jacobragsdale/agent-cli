//! Which REST API a server speaks, and how a credential signs in to it:
//! a bearer token, Airflow 3's JWT, or Airflow 2's HTTP Basic and web form.

use std::time::Duration;

use agent_cli_core::{
    Credential, Exit, Failure, Method, Request, Response, Secret, base64, status_of,
};
use anyhow::{Context, Result, bail};
use serde_json::json;

use super::{Api, Auth, Client};
use crate::refused::refused;

/// How long a server's API version, and that it wants the sign-in form, are
/// remembered; `doctor airflow` asks again every time.
pub(super) const REMEMBER: Duration = Duration::from_secs(3600);

impl Client<'_> {
    /// The instance's `api`, else what the server said within the hour.
    pub(super) fn api(&self) -> Result<Api> {
        if let Some(api) = self.instance.api.or_else(|| self.api.get().copied()) {
            return Ok(api);
        }
        match self.ctx.cache().get(&self.remembered("api")) {
            Some(api) => {
                let _ = self.api.set(api);
                Ok(api)
            }
            None => self.probe(),
        }
    }

    /// Asks the server which API it speaks: `/api/v2/version` answers on
    /// Airflow 3 and is a 404 on 2. It needs no credential, so an agent
    /// wiring an instance never has to know the version.
    pub(crate) fn probe(&self) -> Result<Api> {
        if let Some(api) = self.instance.api {
            return Ok(api);
        }
        let url = format!("{}/api/v2/version", self.instance.base_url);
        let api = match self
            .ctx
            .read(Request::get(url).header("Accept", "application/json"))
        {
            Ok(_) => Api::V2,
            Err(error) if status_of(&error) == Some(404) => Api::V1,
            Err(error) => return Err(refused(error, "version")),
        };
        let _ = self.api.set(api);
        self.ctx
            .cache()
            .put(&self.remembered("api"), &api, REMEMBER);
        Ok(api)
    }

    pub(super) fn remembered(&self, what: &str) -> String {
        format!("airflow.{what} {}", self.instance.base_url)
    }

    /// True when the credential is a `*_kv` one whose value Key Vault has
    /// changed since it was kept, now kept instead: a refused sign-in is then
    /// worth one more try, since `resolve` gives the new value.
    pub(super) fn refreshed(&self) -> Result<bool> {
        match &self.instance.auth {
            Auth::Password { password: kv, .. } | Auth::Token(kv) => {
                Ok(kv.refresh(self.ctx)?.is_some())
            }
            Auth::Missing(_) => Ok(false),
        }
    }

    /// `Basic …` from the username and password.
    pub(super) fn basic(&self) -> Result<Secret> {
        let Auth::Password { username, password } = &self.instance.auth else {
            bail!("no password to send as HTTP Basic");
        };
        let password = password.resolve(self.ctx)?;
        Ok(Secret::new(format!(
            "Basic {}",
            base64(format!("{username}:{}", password.expose()).as_bytes())
        )))
    }

    /// Airflow 2's web sign-in, for an API that takes only session auth:
    /// `GET /login/` for its CSRF token and cookie, then the form. A good
    /// password is a 302 away from `/login/` whose `Set-Cookie` is the
    /// signed-in session; a bad one is a 302 back to it. Reads (they change
    /// nothing), so `--dry-run` still signs in.
    pub(super) fn sign_in_form(&self) -> Result<()> {
        self.sign_in_by_form(true)
    }

    /// The form, and once `again` with a kv password Key Vault has changed.
    fn sign_in_by_form(&self, again: bool) -> Result<()> {
        let Auth::Password { username, password } = &self.instance.auth else {
            return Ok(());
        };
        let login = format!("{}/login/", self.instance.base_url);
        let page = self
            .ctx
            .read(Request::get(&login).header("Accept", "text/html"))
            .map_err(|error| refused(error, "login/"))?;
        let Some(csrf) = csrf_token(&page.body) else {
            return Err(Failure::setup(format!(
                "Airflow refused HTTP Basic, and {login} has no password form: its users sign in through SSO, \
                 so {username} has no password the API takes"
            ))
            .hint(
                "ask the Airflow admins for a service account with a password or a token, \
                 or to add airflow.api.auth.backend.basic_auth to [api] auth_backends",
            )
            .into());
        };
        let form = Request::new(Method::Query, &login)
            .header("Cookie", cookies(&page).expose())
            .form(vec![
                ("username".to_owned(), username.clone()),
                (
                    "password".to_owned(),
                    password.resolve(self.ctx)?.expose().to_owned(),
                ),
                ("csrf_token".to_owned(), csrf),
            ])
            .keep_redirect();
        // ponytail: the cookie stays in memory, so each command signs in, and
        // Flask-AppBuilder's AUTH_RATE_LIMIT allows 5 sign-ins in 40 s.
        let answer = self
            .ctx
            .read(form)
            .map_err(|error| match status_of(&error) {
                Some(429) => Failure::new(
                    Exit::Failed,
                    "Airflow throttled the sign-in: its web server allows a few sign-ins a minute \
                 (AUTH_RATE_LIMIT, 5 per 40 s by default), and its API takes only session auth, \
                 so every command signs in",
                )
                .hint(
                    "wait 40 s and run it again; to lift it, ask the Airflow admins to add \
                 airflow.api.auth.backend.basic_auth to [api] auth_backends",
                )
                .into(),
                _ => refused(error, "login/"),
            })?;
        let signed_in = (300..400).contains(&answer.status)
            && answer
                .header("Location")
                .is_some_and(|to| !to.contains("/login"));
        if !signed_in && again && self.refreshed()? {
            return self.sign_in_by_form(false);
        }
        if !signed_in {
            return Err(Failure::setup(format!(
                "Airflow at {} refused the password for {username}, by its sign-in form and as HTTP Basic",
                self.instance.base_url
            ))
            .hint(format!(
                "check username and the password for instance {:?}; `agent-cli doctor airflow` checks it",
                self.instance.name
            ))
            .into());
        }
        *self.session.borrow_mut() = Some(cookies(&answer));
        Ok(())
    }

    /// `Bearer …`: minted once, and again after a 401.
    pub(super) fn authorization(&self, fresh: bool) -> Result<Secret> {
        if !fresh && let Some(token) = self.token.borrow().clone() {
            return Ok(token);
        }
        let bearer = match &self.instance.auth {
            // After a 401, a kv token Key Vault has changed since is kept,
            // so `resolve` gives the new one.
            Auth::Token(token) => {
                if fresh {
                    self.refreshed()?;
                }
                token.resolve(self.ctx)?
            }
            Auth::Password { username, password } => self.sign_in(username, password)?,
            Auth::Missing(what) => {
                return Err(Failure::setup(format!(
                    "[[airflow.instance]] {:?} has no credential",
                    self.instance.name
                ))
                .hint(format!(
                    "add {what} under it in {}",
                    self.ctx.config().path().display()
                ))
                .into());
            }
        };
        let token = Secret::new(format!("Bearer {}", bearer.expose()));
        *self.token.borrow_mut() = Some(token.clone());
        Ok(token)
    }

    /// A JWT from `/auth/token`. A read (the POST changes nothing), so it
    /// also works under `--dry-run`.
    /// A refused kv password is read from Key Vault again and, when it
    /// changed there, sent once more.
    fn sign_in(&self, username: &str, password: &Credential) -> Result<Secret> {
        let url = format!("{}/auth/token", self.instance.base_url);
        let sign_in = || -> Result<Response> {
            let request = Request::query(
                &url,
                json!({"username": username, "password": password.resolve(self.ctx)?.expose()}),
            )
            .header("Accept", "application/json");
            self.ctx.read(request)
        };
        let response = match sign_in() {
            Err(error)
                if matches!(status_of(&error), Some(400 | 401 | 403)) && self.refreshed()? =>
            {
                sign_in()
            }
            answer => answer,
        };
        let response = response.map_err(|error| {
            match error.downcast::<Failure>() {
                Ok(failure) if matches!(failure.status, Some(400 | 401 | 403)) => {
                    Failure::setup(failure.message)
                        .hint(format!(
                            "check username and the password for instance {:?}; `agent-cli doctor airflow` checks it",
                            self.instance.name
                        ))
                        .into()
                }
                Ok(failure) => refused(failure.into(), "auth/token"),
                Err(error) => error,
            }
        })?;
        let token = response.json()?["access_token"]
            .as_str()
            .filter(|token| !token.is_empty())
            .map(Secret::new)
            .with_context(|| format!("{url} answered without an access_token"))?;
        Ok(token)
    }
}

/// Every `Set-Cookie` of an answer as one `Cookie` value.
fn cookies(response: &Response) -> Secret {
    let pairs: Vec<&str> = response
        .headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
        .filter_map(|(_, value)| value.split(';').next())
        .map(str::trim)
        .collect();
    Secret::new(pairs.join("; "))
}

/// The sign-in form's hidden `csrf_token`, when the page has a password
/// field to go with it.
fn csrf_token(page: &str) -> Option<String> {
    if !page.contains("name=\"password\"") {
        return None;
    }
    let input = page
        .split("<input")
        .find(|input| input.contains("name=\"csrf_token\""))?;
    let value = input.split("value=\"").nth(1)?.split('"').next()?;
    (!value.is_empty()).then(|| value.to_owned())
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::{Answer, FakeTransport, Sent};
    use serde_json::json;

    use crate::testing::{airflow_with, paths};

    const VAULT: &str = "https://kv-contoso.vault.azure.net/secrets/airflow-agent?api-version=7.4";
    const DAGS: &str = "dags?order_by=dag_id&limit=50&offset=0";

    fn config(api: &str, credential: &str) -> String {
        format!(
            "[[airflow.instance]]\nname = \"prod\"\nbase_url = \"https://airflow.contoso.example\"\n\
             api = \"{api}\"\n{credential}_kv = \"kv-contoso/airflow-agent\"\n"
        )
    }

    fn vault(value: &str) -> Answer {
        Answer::json(&json!({"value": value}))
    }

    fn dags() -> Answer {
        Answer::json(&json!({"dags": [], "total_entries": 0}))
    }

    fn forbidden() -> Answer {
        Answer::status(403, r#"{"title": "Forbidden"}"#)
    }

    fn login_page() -> Answer {
        Answer::status(
            200,
            r#"<input name="password"><input name="csrf_token" value="csrf-1">"#,
        )
        .with_header("Set-Cookie", "session=before-1; Path=/")
    }

    fn form(to: &str) -> Answer {
        Answer::status(302, "")
            .with_header("Location", to)
            .with_header("Set-Cookie", "session=signed-in-1; Path=/")
    }

    fn run(
        config: &str,
        answers: Vec<Answer>,
    ) -> (agent_cli_core::testing::Outcome, FakeTransport) {
        airflow_with(config, &["airflow", "dag", "list"], answers)
    }

    fn password(sent: &Sent) -> String {
        match &sent.body {
            Some(body) => body["password"].as_str().unwrap_or_default().to_owned(),
            None => String::new(),
        }
    }

    fn basic(password: &str) -> String {
        format!(
            "Basic {}",
            agent_cli_core::base64(format!("agent:{password}").as_bytes())
        )
    }

    #[test]
    fn a_refused_kv_password_signs_in_again_once_with_what_key_vault_holds_now() {
        let config = format!("{}username = \"agent\"\n", config("v2", "password"));
        let refused = || Answer::status(401, r#"{"detail":"Invalid credentials"}"#);
        let (outcome, transport) = run(
            &config,
            vec![
                vault("rotated-away"),
                refused(),
                vault("rotated-in"),
                Answer::status(201, r#"{"access_token":"jwt-2"}"#),
                dags(),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let sent = transport.sent();
        assert_eq!(
            paths(&transport),
            [
                VAULT,
                "https://airflow.contoso.example/auth/token",
                VAULT,
                "https://airflow.contoso.example/auth/token",
                DAGS
            ]
        );
        assert_eq!(password(&sent[1]), "rotated-away");
        assert_eq!(password(&sent[3]), "rotated-in");
        assert_eq!(sent[4].authorization.as_deref(), Some("Bearer jwt-2"));

        let (outcome, transport) = run(&config, vec![vault("wrong"), refused(), vault("wrong")]);
        assert_eq!(outcome.code, 3, "{outcome:?}");
        assert!(
            outcome.stderr.contains("Invalid credentials"),
            "{}",
            outcome.stderr
        );
        assert_eq!(transport.sent().len(), 3);
    }

    #[test]
    fn a_refused_kv_token_is_sent_again_with_what_key_vault_holds_now() {
        let config = config("v2", "token");
        let refused = || Answer::status(401, r#"{"detail":"Invalid token"}"#);
        let (outcome, transport) = run(
            &config,
            vec![vault("old-token"), refused(), vault("new-token"), dags()],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let sent = transport.sent();
        assert_eq!(sent[1].authorization.as_deref(), Some("Bearer old-token"));
        assert_eq!(sent[3].authorization.as_deref(), Some("Bearer new-token"));

        let (outcome, _) = run(
            &config,
            vec![vault("old-token"), refused(), vault("old-token"), refused()],
        );
        assert_eq!(outcome.code, 3, "{outcome:?}");
        assert!(
            outcome.stderr.contains("Invalid token"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn on_airflow_2_a_refused_kv_password_tries_basic_again_before_the_form() {
        let config = format!("{}username = \"agent\"\n", config("v1", "password"));
        let (outcome, transport) = run(
            &config,
            vec![vault("old-pw"), forbidden(), vault("new-pw"), dags()],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let sent = transport.sent();
        assert_eq!(sent[1].authorization, Some(basic("old-pw")));
        assert_eq!(sent[3].authorization, Some(basic("new-pw")));
        assert_eq!(sent.len(), 4, "no sign-in form");
    }

    #[test]
    fn on_airflow_2_a_form_that_refuses_a_kv_password_is_tried_once_more_when_it_changed() {
        let config = format!("{}username = \"agent\"\n", config("v1", "password"));
        let login = "https://airflow.contoso.example/login/";
        let (outcome, transport) = run(
            &config,
            vec![
                vault("old-pw"),
                forbidden(),
                vault("old-pw"),
                login_page(),
                form("/login/?next=%2Fhome"),
                vault("new-pw"),
                login_page(),
                form("/home"),
                dags(),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            paths(&transport),
            [VAULT, DAGS, VAULT, login, login, VAULT, login, login, DAGS]
        );
        let sent = transport.sent();
        assert_eq!(password(&sent[4]), "old-pw");
        assert_eq!(password(&sent[7]), "new-pw");

        let (outcome, transport) = run(
            &config,
            vec![
                vault("old-pw"),
                forbidden(),
                vault("old-pw"),
                login_page(),
                form("/login/?next=%2Fhome"),
                vault("old-pw"),
            ],
        );
        assert_eq!(outcome.code, 3, "{outcome:?}");
        assert!(
            outcome.stderr.contains("refused the password for agent"),
            "{}",
            outcome.stderr
        );
        assert_eq!(transport.sent().len(), 6);
    }
}
