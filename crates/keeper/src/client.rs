//! Artifact Keeper over REST: the config, the one door every request goes
//! through, paging, and row helpers.
//!
//! The management API is `/api/v1` under the server's address; a PyPI
//! repository's own JSON API (`/pypi/{key}/pypi/...`) is the only other
//! path read, since it is how a proxy reaches its upstream. A token is
//! optional (public repositories read anonymously) and goes as `Bearer`
//! only to URLs under the address.

use std::sync::{Mutex, PoisonError};

use agent_cli_core::{
    Credential, Ctx, Failure, Op, Request, Response, Secret, check_base_url, form_encode,
    percent_encode, same_origin, status_of, utc,
};
use anyhow::{Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};

/// The most rows Keeper sends in one page; it lowers a larger `per_page`
/// to this without saying so.
pub(crate) const PAGE_MAX: usize = 100;

/// `[keeper]` in config.toml.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Section {
    pub(crate) url: Option<String>,
    token: Option<String>,
    token_env: Option<String>,
    token_cmd: Option<String>,
    token_kv: Option<String>,
}

impl Section {
    /// The server's address and host, from config alone.
    pub(crate) fn base(&self) -> Result<(String, String)> {
        let example = "set url = \"https://keeper.contoso.example\" under [keeper] (the address the browser shows, without /api/v1); `agent-cli config example keeper` shows every key";
        let Some(raw) = self
            .url
            .as_deref()
            .map(str::trim)
            .filter(|raw| !raw.is_empty())
        else {
            return Err(Failure::setup("[keeper] url is not set")
                .hint(example)
                .into());
        };
        let base = check_base_url(raw)
            .map_err(|why| Failure::setup(format!("[keeper] url: {why}")).hint(example))?;
        let rest = base.split_once("://").map_or("", |(_, rest)| rest);
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        if path
            .split('/')
            .any(|segment| matches!(segment, "api" | "pypi"))
        {
            return Err(Failure::setup(format!(
                "[keeper] url {raw} is an API or repository address, not the server's"
            ))
            .hint(example)
            .into());
        }
        let host = authority.split(':').next().unwrap_or(authority);
        Ok((base.clone(), host.to_ascii_lowercase()))
    }
}

/// One Artifact Keeper server, and the token for it when there is one.
pub(crate) struct Keeper {
    /// `https://keeper.contoso.example`, no trailing slash.
    pub(crate) base: String,
    token: Option<Credential>,
    /// The `Authorization` value, made on the first request so a
    /// `token_cmd` runs only when something is sent.
    authorization: Mutex<Option<Secret>>,
}

impl Keeper {
    pub(crate) fn load(ctx: &Ctx) -> Result<Self> {
        let section: Section = ctx.section("keeper")?;
        let (base, _) = section.base()?;
        let token = Credential::from_keys(
            "token",
            section.token,
            section.token_env,
            section.token_cmd,
            section.token_kv,
        )
        .map_err(|message| Failure::setup(message).hint(token_hint(&base)))?;
        Ok(Self {
            base,
            token,
            authorization: Mutex::new(None),
        })
    }

    /// An anonymous client for `base`, for tests that need no `Ctx`.
    #[cfg(test)]
    pub(crate) fn anonymous(base: &str) -> Self {
        Self {
            base: base.to_owned(),
            token: None,
            authorization: Mutex::new(None),
        }
    }

    /// Where the token comes from and where it goes, never what it is.
    pub(crate) fn source(&self) -> String {
        match &self.token {
            Some(token) => format!("{} (Bearer) to {}/", token.source(), self.base),
            None => "anonymous: public repositories only".to_owned(),
        }
    }

    pub(crate) fn signed_in(&self) -> bool {
        self.token.is_some()
    }

    /// An `/api/v1` URL.
    pub(crate) fn api(&self, path: &str, query: &[(&str, String)]) -> String {
        let mut url = format!("{}/api/v1{path}", self.base);
        if !query.is_empty() {
            let pairs: Vec<(String, String)> = query
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect();
            url.push('?');
            url.push_str(&form_encode(&pairs));
        }
        url
    }

    pub(crate) fn get(&self, ctx: &Ctx, url: &str) -> Result<Value> {
        self.fetch(ctx, url)?.json()
    }

    /// A read whose answer may be a file rather than JSON.
    pub(crate) fn fetch(&self, ctx: &Ctx, url: &str) -> Result<Response> {
        ctx.read(Signed {
            keeper: self,
            url: url.to_owned(),
        })
        .map_err(|error| self.refused(error))
    }

    /// A repository's details. Keeper answers 404 both for a key it does not
    /// have and for a private repository the caller may not read.
    pub(crate) fn repo(&self, ctx: &Ctx, key: &str) -> Result<Value> {
        match self.get(
            ctx,
            &self.api(&format!("/repositories/{}", segment(key)), &[]),
        ) {
            Err(error) if status_of(&error) == Some(404) => Err(Failure::not_found(format!(
                "no repository {key}{}",
                self.unseen()
            ))
            .hint("agent-cli keeper repo list")
            .into()),
            found => found,
        }
    }

    /// How a 404 might be a private repository: said only when anonymous.
    pub(crate) fn unseen(&self) -> &'static str {
        if self.signed_in() {
            ", or the token may not read it"
        } else {
            ", or it is private (set a token under [keeper] to read private repositories)"
        }
    }

    /// The rows of a page-numbered list (`page`, `per_page`), up to `limit`,
    /// and the total Keeper counted.
    pub(crate) fn pages(
        &self,
        ctx: &Ctx,
        path: &str,
        query: &[(&str, String)],
        limit: usize,
    ) -> Result<(Vec<Value>, Option<u64>)> {
        let per_page = (limit + 1).clamp(1, PAGE_MAX);
        let mut rows = Vec::new();
        let mut page = 1;
        loop {
            let mut asked = query.to_vec();
            asked.push(("page", page.to_string()));
            asked.push(("per_page", per_page.to_string()));
            let answer = self.get(ctx, &self.api(path, &asked))?;
            let items = list(&answer["items"]);
            let total = answer["pagination"]["total"].as_u64();
            let pages = answer["pagination"]["total_pages"].as_u64().unwrap_or(0);
            let empty = items.is_empty();
            rows.extend(items);
            if empty || rows.len() > limit || page as u64 >= pages {
                return Ok((rows, total));
            }
            page += 1;
        }
    }

    /// A repository's files, by Keeper's keyset cursor, up to `limit`
    /// (one more when there are more, so the caller can say so).
    pub(crate) fn artifacts(
        &self,
        ctx: &Ctx,
        key: &str,
        query: &[(&str, String)],
        limit: usize,
    ) -> Result<Vec<Value>> {
        let path = format!("/repositories/{}/artifacts", segment(key));
        let per_page = (limit + 1).clamp(1, PAGE_MAX);
        let mut rows = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut asked = query.to_vec();
            asked.push(("per_page", per_page.to_string()));
            if let Some(cursor) = &cursor {
                asked.push(("cursor", cursor.clone()));
            }
            let answer = match self.get(ctx, &self.api(&path, &asked)) {
                Err(error) if status_of(&error) == Some(404) => {
                    return Err(Failure::not_found(format!(
                        "no repository {key}{}",
                        self.unseen()
                    ))
                    .hint("agent-cli keeper repo list")
                    .into());
                }
                answer => answer?,
            };
            rows.extend(list(&answer["items"]));
            cursor = text(&answer["next_cursor"]);
            if rows.len() > limit || answer["has_more"] != true || cursor.is_none() {
                return Ok(rows);
            }
        }
    }

    /// A 401 says what to set up: a token when there is none, a new one when
    /// it was refused.
    fn refused(&self, mut error: anyhow::Error) -> anyhow::Error {
        if status_of(&error) == Some(401)
            && let Some(failure) = error.downcast_mut::<Failure>()
        {
            failure.hint = Some(if self.signed_in() {
                format!(
                    "the token was refused: it may have expired or been revoked; make one at {}/access-tokens, then run agent-cli doctor keeper",
                    self.base
                )
            } else {
                format!(
                    "this needs a sign-in (a private repository, or a server without anonymous reads): {}",
                    token_hint(&self.base)
                )
            });
        }
        error
    }

    fn authorization(&self, ctx: &Ctx, fresh: bool) -> Result<Secret> {
        let mut held = self
            .authorization
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if !fresh && let Some(held) = held.as_ref() {
            return Ok(held.clone());
        }
        let Some(token) = &self.token else {
            bail!("no Keeper token is set up");
        };
        // Refused: a `token_kv` reads Key Vault again; every other source
        // (and a vault value that did not change) resolves as before.
        let value = match fresh {
            true => token.refresh(ctx)?,
            false => None,
        };
        let value = match value {
            Some(value) => value,
            None => token.resolve(ctx)?,
        };
        let secret = Secret::new(format!("Bearer {}", value.expose()));
        *held = Some(secret.clone());
        Ok(secret)
    }
}

fn token_hint(base: &str) -> String {
    format!(
        "make an API token at {base}/access-tokens (your profile, Access Tokens) and set token_env = \"KEEPER_TOKEN\" (or token_cmd) under [keeper]"
    )
}

/// A GET with the token, attached when it is performed and only under the
/// server's address.
struct Signed<'a> {
    keeper: &'a Keeper,
    url: String,
}

impl Signed<'_> {
    fn request<'r>(&self) -> Request<'r> {
        Request::get(self.url.clone()).header("Accept", "application/json")
    }
}

impl Op for Signed<'_> {
    type Output = Response;

    fn plan(&self) -> Value {
        let mut plan = self.request().plan();
        if self.keeper.signed_in() {
            plan["headers"]["Authorization"] = json!("***");
        }
        plan
    }

    fn writes(&self) -> bool {
        false
    }

    fn perform(self, ctx: &Ctx) -> Result<Response> {
        let base = &self.keeper.base;
        if !same_origin(base, &self.url) {
            bail!(
                "refusing to send a Keeper request to {}; they go only under {base}/",
                self.url
            );
        }
        if !self.keeper.signed_in() {
            return self.request().perform(ctx);
        }
        let mint = |fresh: bool| self.keeper.authorization(ctx, fresh);
        self.request().auth(&mint).perform(ctx)
    }
}

/// A path's segments, each escaped, joined by `/`.
pub(crate) fn segment(raw: &str) -> String {
    raw.split('/')
        .map(|part| {
            let mut out = String::new();
            percent_encode(part, &mut out);
            out.replace('+', "%20")
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// A PyPI project name as PEP 503 compares them: lowercase, each run of
/// `-`, `_` and `.` one `-`.
pub(crate) fn normalize(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if matches!(c, '-' | '_' | '.') {
            if !out.ends_with('-') {
                out.push('-');
            }
        } else {
            out.push(c.to_ascii_lowercase());
        }
    }
    out
}

// ---------- rows ----------

pub(crate) fn list(value: &Value) -> Vec<Value> {
    value.as_array().cloned().unwrap_or_default()
}

/// A string field, `None` when absent or empty.
pub(crate) fn text(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// A service's time as RFC 3339 UTC, whole seconds.
pub(crate) fn stamp(value: &Value) -> Option<String> {
    value.as_str().filter(|raw| !raw.is_empty()).map(utc)
}

/// A stamp to sort by: whole seconds first, then Keeper's own fraction,
/// since two uploads of one pipeline step often land in the same second.
pub(crate) fn moment(value: &Value) -> (Option<String>, String) {
    (stamp(value), value.as_str().unwrap_or_default().to_owned())
}

/// `quarantine_status` when it says something: Keeper holds a quarantined
/// file back from downloads.
pub(crate) fn quarantine(file: &Value) -> Option<String> {
    text(&file["quarantine_status"]).filter(|status| status != "not_quarantined")
}

/// The note a cut list prints.
pub(crate) fn note_more(ctx: &Ctx, shown: usize, total: Option<u64>) {
    match total {
        Some(total) => ctx.note(format!("[{shown} of {total}; --limit N]")),
        None => ctx.note(format!("[first {shown}; --limit N for more]")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_url_is_the_servers_address_and_never_the_apis() {
        let base = |url: &str| {
            Section {
                url: Some(url.to_owned()),
                ..Section::default()
            }
            .base()
        };
        assert_eq!(
            base("https://Keeper.contoso.example/").unwrap(),
            (
                "https://Keeper.contoso.example".to_owned(),
                "keeper.contoso.example".to_owned()
            )
        );
        assert!(base("http://127.0.0.1:18070").is_ok());
        for wrong in [
            "http://keeper.contoso.example",
            "https://keeper.contoso.example/api/v1",
            "https://keeper.contoso.example/pypi/pypi/simple",
        ] {
            assert!(base(wrong).is_err(), "{wrong}");
        }
    }

    #[test]
    fn names_compare_as_pep_503_says_and_paths_keep_their_slashes() {
        assert_eq!(normalize("Contoso_Orders.Client"), "contoso-orders-client");
        assert_eq!(normalize("a--_b"), "a-b");
        assert_eq!(
            segment("api/1.4.2/api 1.4.2+linux.tar.gz"),
            "api/1.4.2/api%201.4.2%2Blinux.tar.gz"
        );
    }

    /// `repo list` with `token_kv` over these answers: its exit and each
    /// request's `Authorization`.
    fn with_token_kv(answers: Vec<agent_cli_core::testing::Answer>) -> (u8, Vec<Option<String>>) {
        let transport = agent_cli_core::testing::FakeTransport::answering(answers);
        let config = "[keeper]\nurl = \"https://keeper.contoso.example\"\ntoken_kv = \"kv-contoso/keeper-token\"\n";
        let setup = agent_cli_core::Setup::fake(transport.clone()).with_config(config);
        let outcome =
            agent_cli_core::testing::run(&[crate::DOMAIN], &["keeper", "repo", "list"], setup);
        let sent = transport.sent().into_iter();
        (outcome.code, sent.map(|sent| sent.authorization).collect())
    }

    #[test]
    fn a_token_kv_refused_is_read_again_from_key_vault_and_sent_once_more() {
        use agent_cli_core::testing::Answer;
        let vault = |value: &str| Answer::json(&json!({ "value": value }));
        let empty = || crate::testing::page(vec![], 0);
        let refused = || Answer::status(401, "Authentication required");
        let (code, sent) = with_token_kv(vec![vault("old"), refused(), vault("new"), empty()]);
        assert_eq!(code, 0, "{sent:?}");
        assert_eq!(sent[1].as_deref(), Some("Bearer old"));
        assert_eq!(sent[3].as_deref(), Some("Bearer new"));

        let (code, sent) = with_token_kv(vec![vault("old"), refused(), vault("old"), refused()]);
        assert_eq!(code, 3, "unchanged, the refusal stands: {sent:?}");
    }

    #[test]
    fn a_refusal_without_a_token_names_how_to_make_one() {
        let (outcome, _) = crate::testing::anonymous(
            &["keeper", "repo", "list"],
            vec![agent_cli_core::testing::Answer::status(
                401,
                "Authentication required",
            )],
        );
        assert_eq!(outcome.code, 3, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("https://keeper.contoso.example/access-tokens"),
            "{}",
            outcome.stderr
        );
    }
}
