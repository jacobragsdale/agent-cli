//! `lake`: Azure Data Lake Storage Gen2, the storage accounts with a
//! hierarchical namespace, over the `dfs` endpoint with the az login's
//! storage token. Every account the inventory finds is in reach unless
//! `[azure] storage_accounts` narrows it.
//!
//! An account's id is its name, a container's `account/container`, a path's
//! `account/container/dir/file.parquet` (a directory the same, no trailing
//! slash). [`Ref::parse`] also takes `abfss://` and `abfs://`
//! (`container@account.dfs.core.windows.net/…`), `wasbs://`, the `https://`
//! URL on `dfs` or `blob`, and Snowflake's `azure://` stage URL, read and
//! percent-decoded, never fetched; a SAS on any of them is dropped.
//!
//! [`Lake`] is the door: the token goes to `{account}.dfs.core.windows.net`
//! for an account in reach and nowhere else, checked on every URL, since a
//! ref names its host. `list` reads Path List pages under the deadline;
//! `parquet.rs` reads a file's footer by ranged GETs and DuckDB, and
//! downloads files for `query run`; `table/mod.rs` finds tables and their
//! partitions in a listing.

pub(crate) mod access;
pub(crate) mod account;
pub(crate) mod container;
pub(crate) mod parquet;
pub(crate) mod partition;
pub(crate) mod path;
pub(crate) mod table;
#[cfg(test)]
pub(crate) mod testing;

use std::time::Duration;

use agent_cli_core::{Ctx, Failure, Method, Request, Response, host_under};
use anyhow::{Result, bail};
use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc2822;

use crate::client::{Kind, STORAGE, bearer, decode, narrow, text};
use crate::config::Azure;
use crate::graph::{StorageAccount, inventory};

/// A version the Data Lake spec (azure-rest-api-specs, stable 2026-06-06)
/// lists; Path List's JSON and the HEADs' headers read here are the same in
/// every version since 2019-12-12.
// VERIFY(work): that the accounts take it (every public-cloud account takes
// every globally enabled version).
const VERSION: &str = "2025-01-05";
/// The most entries one Path List page holds.
const PAGE: usize = 5000;
/// What a paged listing leaves of the deadline for the command's own work.
const MARGIN: Duration = Duration::from_secs(5);

pub(crate) const ACCOUNTS: Kind = Kind {
    noun: "storage account",
    key: "storage_accounts",
    flag: "an id's account",
    domain: "lake",
};

/// What a lake id names: an account, a container in it, or a path in that.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Ref {
    pub account: String,
    /// Empty for the account itself.
    pub container: String,
    /// Within the container, without a leading or trailing `/`; empty for
    /// its root.
    pub path: String,
}

impl Ref {
    /// Reads any of the forms the module doc names.
    pub fn parse(raw: &str) -> Result<Self> {
        let raw = raw.trim();
        let bad = || -> anyhow::Error {
            Failure::usage(format!("{raw:?} is not a lake id"))
                .hint("pass ACCOUNT/CONTAINER/PATH or its abfss://, wasbs://, https:// or azure:// URL; agent-cli lake account list names the accounts")
                .into()
        };
        // A SAS or a fragment is never part of what is named.
        let bare = raw.split(['?', '#']).next().unwrap_or_default();
        let (account, container, path) = match bare.split_once("://") {
            None => {
                let (account, rest) = bare.split_once('/').unwrap_or((bare, ""));
                let (container, path) = rest.split_once('/').unwrap_or((rest, ""));
                (account.to_owned(), container, path)
            }
            Some((scheme, rest)) => match scheme.to_ascii_lowercase().as_str() {
                "abfss" | "abfs" | "wasbs" | "wasb" => {
                    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
                    let (container, host) = authority.split_once('@').ok_or_else(bad)?;
                    (account_of(host).ok_or_else(bad)?, container, path)
                }
                "https" | "azure" => {
                    let (host, rest) = rest.split_once('/').unwrap_or((rest, ""));
                    let (container, path) = rest.split_once('/').unwrap_or((rest, ""));
                    (account_of(host).ok_or_else(bad)?, container, path)
                }
                _ => return Err(bad()),
            },
        };
        let account = account.to_ascii_lowercase();
        let container = decode(container).to_ascii_lowercase();
        let name_ok = |name: &str, allowed: fn(&u8) -> bool| {
            (3..=63).contains(&name.len()) && name.as_bytes().iter().all(allowed)
        };
        if !name_ok(&account, u8::is_ascii_alphanumeric)
            || account.len() > 24
            || (!container.is_empty()
                && !name_ok(&container, |b| b.is_ascii_alphanumeric() || *b == b'-'))
        {
            return Err(bad());
        }
        let path: Vec<String> = path
            .split('/')
            .filter(|segment| !segment.is_empty())
            .map(decode)
            .collect();
        if container.is_empty() && !path.is_empty() {
            return Err(bad());
        }
        Ok(Self {
            account,
            container,
            path: path.join("/"),
        })
    }

    /// `account[/container[/path]]`: what every lake verb takes.
    pub fn id(&self) -> String {
        [self.account.as_str(), &self.container, &self.path]
            .iter()
            .filter(|part| !part.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join("/")
    }

    /// The same container, at `path`.
    pub fn at(&self, path: &str) -> Self {
        Self {
            path: path.trim_matches('/').to_owned(),
            ..self.clone()
        }
    }

    /// The folder holding it; `None` at the container's root.
    pub fn parent(&self) -> Option<Self> {
        if self.path.is_empty() {
            return None;
        }
        Some(self.at(self.path.rsplit_once('/').map_or("", |(parent, _)| parent)))
    }

    /// Its URL on the `dfs` endpoint: the account, the container, or the path.
    pub fn url(&self) -> String {
        let mut url = format!("https://{}.dfs.core.windows.net/", self.account);
        if !self.container.is_empty() {
            url.push_str(&self.container);
            if !self.path.is_empty() {
                url.push('/');
                url.push_str(&encoded(&self.path, true));
            }
        }
        url
    }

    /// The container root's URL with its trailing slash, as a HEAD on the
    /// root directory takes it; any other path's own URL.
    // VERIFY(work): that `HEAD /{container}/` (with ?action=getAccessControl)
    // answers for the root directory.
    pub fn head_url(&self) -> String {
        if self.path.is_empty() {
            format!("{}/", self.url())
        } else {
            self.url()
        }
    }
}

/// The account a `dfs` or `blob` host names.
fn account_of(host: &str) -> Option<String> {
    let host = host.to_ascii_lowercase();
    [".dfs.core.windows.net", ".blob.core.windows.net"]
        .iter()
        .find_map(|suffix| host.strip_suffix(suffix))
        .filter(|account| !account.is_empty() && !account.contains('.'))
        .map(str::to_owned)
}

/// Percent-encoded for a URL: everything but RFC 3986's unreserved set, a
/// space as `%20` (a `+` in a path is a plus), and `/` kept in a path.
pub(crate) fn encoded(raw: &str, keep_slash: bool) -> String {
    let mut out = String::new();
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            b'/' if keep_slash => out.push('/'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// A storage stamp, `Mon, 29 Sep 2026 06:02:11 GMT`.
pub(crate) fn http_time(raw: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(raw.trim(), &Rfc2822).ok()
}

/// A size as people read it: `4.1 GiB`.
pub(crate) fn bytes(size: u64) -> String {
    let units = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = size as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < units.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{size} B")
    } else {
        format!("{value:.1} {}", units[unit])
    }
}

/// One entry of a Path List.
#[derive(Clone, Debug, Default)]
pub(crate) struct Entry {
    /// Within the container.
    pub path: String,
    pub directory: bool,
    pub size: u64,
    pub modified: Option<OffsetDateTime>,
    pub owner: Option<String>,
}

impl Entry {
    fn read(held: &Value) -> Option<Self> {
        // Path List writes numbers and flags as strings.
        let number = |value: &Value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|raw| raw.parse().ok()))
        };
        Some(Self {
            path: text(&held["name"])?,
            directory: held["isDirectory"] == "true" || held["isDirectory"] == true,
            size: number(&held["contentLength"]).unwrap_or(0),
            modified: held["lastModified"].as_str().and_then(http_time),
            owner: text(&held["owner"]),
        })
    }
}

/// A listing, and whether the deadline let it finish.
pub(crate) struct Listing {
    pub entries: Vec<Entry>,
    pub complete: bool,
}

/// The door to the accounts in reach.
pub(crate) struct Lake<'c> {
    ctx: &'c Ctx,
    pub azure: Azure,
    pub accounts: Vec<StorageAccount>,
}

impl<'c> Lake<'c> {
    pub fn load(ctx: &'c Ctx) -> Result<Self> {
        let azure = Azure::load(ctx)?;
        let accounts = narrow(
            &inventory(ctx, &azure)?.storage_accounts,
            &azure.storage_accounts,
            &[],
            ACCOUNTS,
            |account| &account.id,
        )?;
        Ok(Self {
            ctx,
            azure,
            accounts,
        })
    }

    pub fn ctx(&self) -> &'c Ctx {
        self.ctx
    }

    /// The account in reach named `name`, else exit 2 naming the ones there.
    pub fn account(&self, name: &str) -> Result<&StorageAccount> {
        self.accounts
            .iter()
            .find(|held| held.id.eq_ignore_ascii_case(name))
            .ok_or_else(|| {
                let names: Vec<&str> = self.accounts.iter().map(|held| held.id.as_str()).collect();
                Failure::usage(format!(
                    "the storage account {name} is not in reach (in reach: {})",
                    if names.is_empty() {
                        "none".to_owned()
                    } else {
                        names.join(", ")
                    }
                ))
                .hint("`agent-cli lake account list` names the accounts the login reaches")
                .into()
            })
    }

    /// `raw` read as an id, its account in reach; with `container`, one
    /// that names at least a container.
    pub fn resolve(&self, raw: &str, container: bool) -> Result<Ref> {
        let found = Ref::parse(raw)?;
        self.account(&found.account)?;
        if container && found.container.is_empty() {
            return Err(Failure::usage(format!(
                "{raw} names an account, not a container or a path"
            ))
            .hint(format!(
                "`agent-cli lake container list {}` names its containers",
                found.account
            ))
            .into());
        }
        Ok(found)
    }

    /// `url`, when it is the `dfs` endpoint of an account in reach.
    fn signed<'u>(&self, url: &'u str) -> Result<&'u str> {
        let host = url
            .strip_prefix("https://")
            .and_then(|rest| rest.split(['/', '?', '#']).next())
            .unwrap_or_default();
        let ours = host_under(url, ".dfs.core.windows.net")
            && account_of(host).is_some_and(|account| {
                self.accounts
                    .iter()
                    .any(|held| held.id.eq_ignore_ascii_case(&account))
            });
        if !ours {
            bail!(
                "{url} is not the Data Lake of an account in reach; the storage token is not sent there"
            );
        }
        Ok(url)
    }

    /// One signed read, `range` an inclusive byte span.
    fn send(&self, method: Method, url: &str, range: Option<(u64, u64)>) -> Result<Response> {
        let mint = bearer(self.ctx, STORAGE);
        let mut request = Request::new(method, self.signed(url)?)
            .header("x-ms-version", VERSION)
            .auth(&mint);
        if let Some((start, end)) = range {
            request = request.header("Range", format!("bytes={start}-{end}"));
        }
        self.ctx.read(request).map_err(|error| explain(url, error))
    }

    pub fn get(&self, url: &str) -> Result<Response> {
        self.send(Method::Get, url, None)
    }

    pub fn head(&self, url: &str) -> Result<Response> {
        self.send(Method::Head, url, None)
    }

    /// Bytes `start..=end` of the file at `url`.
    pub fn range(&self, url: &str, start: u64, end: u64) -> Result<Vec<u8>> {
        Ok(self
            .send(Method::Get, url, Some((start, end)))?
            .into_bytes())
    }

    /// The containers of `account`, with when each last changed.
    pub fn containers(&self, account: &str) -> Result<Vec<(String, Option<OffsetDateTime>)>> {
        let base = format!("https://{account}.dfs.core.windows.net/?resource=account");
        let mut found = Vec::new();
        for page in self.pages(&base)?.0 {
            for held in page["filesystems"].as_array().into_iter().flatten() {
                if let Some(name) = text(&held["name"]) {
                    found.push((name, held["lastModified"].as_str().and_then(http_time)));
                }
            }
        }
        Ok(found)
    }

    /// What is under `at` (a container or a folder), one level or all.
    pub fn list(&self, at: &Ref, recursive: bool) -> Result<Listing> {
        let mut base = format!(
            "https://{}.dfs.core.windows.net/{}?resource=filesystem&recursive={recursive}&maxResults={PAGE}",
            at.account, at.container
        );
        if !at.path.is_empty() {
            base.push_str(&format!("&directory={}", encoded(&at.path, false)));
        }
        let (pages, complete) = self.pages(&base)?;
        let entries = pages
            .iter()
            .flat_map(|page| page["paths"].as_array().cloned().unwrap_or_default())
            .filter_map(|held| Entry::read(&held))
            .collect();
        Ok(Listing { entries, complete })
    }

    /// Each page from `base`, following `x-ms-continuation`, until there are
    /// no more or the deadline is near (then `false`).
    fn pages(&self, base: &str) -> Result<(Vec<Value>, bool)> {
        let mut pages = Vec::new();
        let mut url = base.to_owned();
        loop {
            let answer = self.get(&url)?;
            let next = answer.header("x-ms-continuation").map(str::to_owned);
            pages.push(answer.json()?);
            let Some(token) = next.filter(|token| !token.is_empty()) else {
                return Ok((pages, true));
            };
            if self.ctx.remaining().unwrap_or_default() < MARGIN {
                return Ok((pages, false));
            }
            url = format!("{base}&continuation={}", encoded(&token, false));
        }
    }
}

/// The refusals worth saying in the lake's words, with the command that
/// tells more.
fn explain(url: &str, error: anyhow::Error) -> anyhow::Error {
    let status = crate::client::refused_with(&error);
    let said = format!("{error:#}");
    let mut failure = match error.downcast::<Failure>() {
        Ok(failure) => failure,
        Err(error) => return error,
    };
    // A listing names its folder in `directory=`.
    let named = Ref::parse(url).ok().map(|found| {
        let query = url.split_once('?').map_or("", |(_, query)| query);
        match query
            .split('&')
            .find_map(|pair| pair.strip_prefix("directory="))
        {
            Some(directory) => found.at(&decode(directory)),
            None => found,
        }
    });
    let id = named.as_ref().map(Ref::id).unwrap_or_default();
    match status {
        Some(403)
            if said.contains("AuthorizationPermissionMismatch")
                || said.contains("using this permission") =>
        {
            failure.message.push_str(
                " (the login holds no data role here and no ACL lets it in; Owner or Contributor on the account is control plane only)",
            );
            failure.hint = Some(format!(
                "`agent-cli lake access get {id} --principal @me` shows where it breaks"
            ));
        }
        Some(403)
            if said.contains("AuthorizationFailure")
                || said.contains("not authorized to perform this operation.") =>
        {
            failure
                .message
                .push_str(" (the account's firewall refused this address)");
            failure.hint = Some(
                "`agent-cli net endpoint list` shows the private endpoints that reach it"
                    .to_owned(),
            );
        }
        Some(404) => {
            failure.hint = named.map(|found| match found.parent() {
                Some(parent) => format!(
                    "`agent-cli lake path list {}` shows what is there",
                    parent.id()
                ),
                None => format!(
                    "`agent-cli lake container list {}` shows what is there",
                    found.account
                ),
            });
        }
        _ => {}
    }
    failure.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(raw: &str) -> String {
        Ref::parse(raw).unwrap().id()
    }

    #[test]
    fn every_url_form_names_the_same_path_and_a_sas_is_dropped() {
        let id = "contosolake/raw/sales/orders/date=2026-09-29/part 1.parquet";
        for raw in [
            id,
            "abfss://raw@contosolake.dfs.core.windows.net/sales/orders/date=2026-09-29/part%201.parquet",
            "abfs://raw@ContosoLake.dfs.core.windows.net/sales/orders/date=2026-09-29/part%201.parquet",
            "wasbs://raw@contosolake.blob.core.windows.net/sales/orders/date%3D2026-09-29/part%201.parquet",
            "https://contosolake.dfs.core.windows.net/raw/sales/orders/date=2026-09-29/part%201.parquet",
            "https://contosolake.blob.core.windows.net/raw/sales//orders/date=2026-09-29/part%201.parquet?sv=2024&sig=secret",
            "azure://contosolake.blob.core.windows.net/raw/sales/orders/date=2026-09-29/part%201.parquet",
        ] {
            assert_eq!(parsed(raw), id, "{raw}");
        }
        assert_eq!(parsed("contosolake/raw/"), "contosolake/raw");
        assert_eq!(
            parsed("abfss://raw@contosolake.dfs.core.windows.net/"),
            "contosolake/raw"
        );
        assert_eq!(parsed("contosolake"), "contosolake");
        let found = Ref::parse(id).unwrap();
        assert_eq!(
            found.url(),
            "https://contosolake.dfs.core.windows.net/raw/sales/orders/date%3D2026-09-29/part%201.parquet"
        );
        assert_eq!(
            found.parent().unwrap().id(),
            "contosolake/raw/sales/orders/date=2026-09-29"
        );
        assert_eq!(
            Ref::parse("contosolake/raw").unwrap().head_url(),
            "https://contosolake.dfs.core.windows.net/raw/"
        );
    }

    #[test]
    fn a_host_that_is_not_storage_or_a_bad_name_is_exit_2() {
        for raw in [
            "https://contosolake.vault.azure.net/raw/x",
            "abfss://contosolake.dfs.core.windows.net/x",
            "s3://bucket/x",
            "Contoso_Lake/raw",
            "contosolake/Raw_Zone/x",
            "ab/raw",
            "",
        ] {
            let error = Ref::parse(raw).unwrap_err();
            let failure = error.downcast_ref::<Failure>().unwrap();
            assert_eq!(failure.exit, agent_cli_core::Exit::Usage, "{raw}");
        }
    }

    #[test]
    fn a_firewall_refusal_names_the_endpoints_and_another_403_keeps_its_words() {
        let refused = |said: &str| {
            let mut failure = Failure::new(agent_cli_core::Exit::Failed, said);
            failure.status = Some(403);
            let error = explain(
                "https://contosolake.dfs.core.windows.net/raw/x",
                failure.into(),
            );
            let failure = error.downcast::<Failure>().unwrap();
            (failure.message, failure.hint)
        };
        let (message, hint) = refused("GET … answered 403: AuthorizationFailure");
        assert!(message.ends_with("(the account's firewall refused this address)"));
        assert!(hint.unwrap().contains("agent-cli net endpoint list"));
        let (message, hint) = refused("GET … answered 403: AuthenticationFailed");
        assert_eq!(
            (message.as_str(), hint),
            ("GET … answered 403: AuthenticationFailed", None)
        );
    }

    #[test]
    fn a_storage_stamp_and_a_size_read_as_people_read_them() {
        assert_eq!(
            http_time("Mon, 29 Sep 2026 06:02:11 GMT")
                .map(agent_cli_core::utc_time)
                .as_deref(),
            Some("2026-09-29T06:02:11Z")
        );
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(4_402_341_478), "4.1 GiB");
        assert_eq!(encoded("sales/date=1 2", false), "sales%2Fdate%3D1%202");
    }
}
