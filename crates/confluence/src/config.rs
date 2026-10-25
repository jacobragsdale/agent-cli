//! `[confluence]`: the wiki's address and the token's keys. The address is
//! the one people browse, context path included, and every call goes under
//! it.

use agent_cli_core::{Failure, check_base_url, host_under};
use anyhow::Result;
use serde::Deserialize;

/// `[confluence]` in config.toml.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Section {
    pub(crate) url: Option<String>,
    pub(crate) token: Option<String>,
    pub(crate) token_env: Option<String>,
    pub(crate) token_cmd: Option<String>,
}

impl Section {
    /// The site, from config alone: for the overview.
    pub(crate) fn site(&self) -> Result<Site> {
        Site::parse(self.url.as_deref())
    }
}

/// The wiki people browse: where web URLs point and API calls go, parsed
/// but never fetched.
#[derive(Debug)]
pub(crate) struct Site {
    /// `https://wiki.contoso.example`, or `https://contoso.example/confluence`
    /// behind a context path; no trailing slash.
    pub(crate) base: String,
    pub(crate) host: String,
    /// `/confluence`, or empty.
    pub(crate) context: String,
}

impl Site {
    /// `https://HOST[:PORT][/CONTEXT]` (plain http only to this machine).
    /// A Cloud site (`*.atlassian.net`) is exit 3: its API is another one.
    pub(crate) fn parse(raw: Option<&str>) -> Result<Self> {
        let example = "set url = \"https://wiki.contoso.example\" under [confluence] (the address the browser shows, before /display or /spaces); `agent-cli config example confluence` shows every key";
        let Some(raw) = raw.map(str::trim).filter(|raw| !raw.is_empty()) else {
            return Err(Failure::setup("[confluence] url is not set")
                .hint(example)
                .into());
        };
        if host_under(raw, "atlassian.net") {
            return Err(Failure::setup(format!(
                "{raw} is Confluence Cloud, and this domain speaks Confluence Data Center and Server"
            ))
            .hint("point url at your self-hosted wiki")
            .into());
        }
        let base = check_base_url(raw)
            .map_err(|why| Failure::setup(format!("[confluence] url: {why}")).hint(example))?;
        let rest = base.split_once("://").map_or("", |(_, rest)| rest);
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        let page = path.split('/').any(|segment| {
            matches!(segment, "rest" | "display" | "spaces" | "pages" | "x")
                || segment.ends_with(".action")
        });
        if page {
            return Err(Failure::setup(format!(
                "[confluence] url {raw} is a page's or the API's address, not the wiki's"
            ))
            .hint(example)
            .into());
        }
        let host = authority
            .split(':')
            .next()
            .unwrap_or(authority)
            .to_ascii_lowercase();
        Ok(Self {
            context: if path.is_empty() {
                String::new()
            } else {
                format!("/{path}")
            },
            host,
            base,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_site_is_a_self_hosted_wiki_with_or_without_a_context_path() {
        let site = Site::parse(Some("https://Wiki.contoso.example/")).unwrap();
        assert_eq!(
            (
                site.base.as_str(),
                site.host.as_str(),
                site.context.as_str()
            ),
            ("https://Wiki.contoso.example", "wiki.contoso.example", "")
        );
        let site = Site::parse(Some("https://contoso.example:8443/confluence")).unwrap();
        assert_eq!(
            (
                site.base.as_str(),
                site.host.as_str(),
                site.context.as_str()
            ),
            (
                "https://contoso.example:8443/confluence",
                "contoso.example",
                "/confluence"
            )
        );
        assert!(Site::parse(Some("http://localhost:8090/confluence")).is_ok());
        for raw in [
            "http://wiki.contoso.example",
            "https://wiki.contoso.example/display/ENG/Home",
            "https://wiki.contoso.example/confluence/rest/api",
            "https://wiki.contoso.example/pages/viewpage.action?pageId=1",
        ] {
            assert!(Site::parse(Some(raw)).is_err(), "{raw}");
        }
        let cloud = Site::parse(Some("https://contoso.atlassian.net/wiki")).unwrap_err();
        assert!(cloud.to_string().contains("is Confluence Cloud"), "{cloud}");
    }
}
