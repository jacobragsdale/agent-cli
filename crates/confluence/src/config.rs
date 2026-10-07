//! `[confluence]`: the site and the credential's keys. The site is where
//! web URLs point; whether calls go there or through Atlassian's gateway is
//! the client's choice.

use agent_cli_core::{Failure, host_under};
use anyhow::Result;
use serde::Deserialize;

/// `[confluence]` in config.toml.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Section {
    pub(crate) url: Option<String>,
    pub(crate) email: Option<String>,
    pub(crate) token: Option<String>,
    pub(crate) token_env: Option<String>,
    pub(crate) token_cmd: Option<String>,
    pub(crate) cloud_id: Option<String>,
}

impl Section {
    /// The site, from config alone: for the overview.
    pub(crate) fn site(&self) -> Result<Site> {
        Site::parse(self.url.as_deref(), self.cloud_id.is_some())
    }
}

/// The site people browse: where web URLs point, parsed but never fetched.
#[derive(Debug)]
pub(crate) struct Site {
    /// `https://contoso.atlassian.net`
    pub(crate) web: String,
    pub(crate) host: String,
}

impl Site {
    /// `https://SITE.atlassian.net[/wiki]`. Any other host is Data Center or
    /// Server, which this domain does not speak (exit 3), unless a
    /// `cloud_id` says it is a Cloud site on a custom domain.
    pub(crate) fn parse(raw: Option<&str>, cloud_id: bool) -> Result<Self> {
        let example = "set url = \"https://SITE.atlassian.net/wiki\" under [confluence]; `agent-cli config example confluence` shows every key";
        let Some(raw) = raw.map(str::trim).filter(|raw| !raw.is_empty()) else {
            return Err(Failure::setup("[confluence] url is not set")
                .hint(example)
                .into());
        };
        let Some(rest) = raw.strip_prefix("https://") else {
            return Err(
                Failure::setup(format!("[confluence] url {raw} is not https://"))
                    .hint(example)
                    .into(),
            );
        };
        let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
        let host = host.to_ascii_lowercase();
        if !matches!(path.trim_end_matches('/'), "" | "wiki") || !host_under(raw, &host) {
            return Err(
                Failure::setup(format!("[confluence] url {raw} is not a site's address"))
                    .hint(example)
                    .into(),
            );
        }
        if !cloud_id && !host_under(raw, ".atlassian.net") {
            return Err(Failure::setup(format!(
                "{host} is not a Confluence Cloud site (SITE.atlassian.net): Confluence Data Center and Server are not supported"
            ))
            .hint("point url at a Cloud site; one on a custom domain also needs cloud_id")
            .into());
        }
        Ok(Self {
            web: format!("https://{host}"),
            host,
        })
    }

    /// `contoso`, for the overview.
    pub(crate) fn name(&self) -> &str {
        self.host.split('.').next().unwrap_or(&self.host)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_site_is_cloud_unless_a_cloud_id_vouches_for_another_host() {
        for raw in [
            "https://contoso.atlassian.net",
            "https://Contoso.atlassian.net/wiki/",
        ] {
            let site = Site::parse(Some(raw), false).unwrap();
            assert_eq!(
                (site.web.as_str(), site.name()),
                ("https://contoso.atlassian.net", "contoso")
            );
        }
        for raw in [
            "http://contoso.atlassian.net",
            "https://contoso.atlassian.net/wiki/spaces/ENG",
            "https://wiki.contoso.example/confluence",
        ] {
            assert!(Site::parse(Some(raw), false).is_err(), "{raw}");
        }
        let error = Site::parse(Some("https://wiki.contoso.example"), false).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Data Center and Server are not supported"),
            "{error}"
        );
        assert!(Site::parse(Some("https://wiki.contoso.example"), true).is_ok());
    }
}
