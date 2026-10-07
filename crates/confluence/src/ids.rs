//! What commands take to name a page, a space, a comment or an attachment:
//! the id rows print, or the web URL a person pastes, parsed and never
//! fetched. A URL on another host is exit 2.
//!
//! Page URLs come in many forms (`/spaces/KEY/pages/ID/slug`, the editor's
//! `edit-v2`, blog posts by date, `viewpage.action`, `resumedraft.action`,
//! `/display/KEY/Title`) and tiny links, `/x/CODE`, which are the id as 8
//! little-endian bytes in base64 with `/` as `-`, `+` as `_` and the
//! padding and trailing `A`s dropped: decoded here, so no redirect is
//! followed. Share links (`/l/cp/…`) are server-side and cannot be read.

use agent_cli_core::{Ctx, Failure};
use anyhow::Result;

use crate::client::{Confluence, text};
use crate::config::Site;

/// A page as a command was given it.
#[derive(Debug, PartialEq)]
pub(crate) enum PageRef {
    Id {
        id: String,
        version: Option<u64>,
    },
    /// `KEY:Title`, or `/display/KEY/Title`: looked up by exact title, any case.
    Title {
        space: String,
        title: String,
    },
    /// A space's URL: its homepage.
    Space(String),
}

impl PageRef {
    pub(crate) fn parse(site: &Site, raw: &str) -> Result<Self> {
        let raw = raw.trim();
        let wrong = |why: String| -> anyhow::Error {
            Failure::usage(why)
                .hint("pass the page's id, ID@VERSION, KEY:Title or its URL; agent-cli confluence page list TEXT finds it")
                .into()
        };
        if let Some((path, query)) = web(site, raw)? {
            return page_url(&path, &query)
                .ok_or_else(|| wrong(format!("{raw} is not a page URL")));
        }
        if let Some((id, version)) = raw.split_once('@')
            && is_id(id)
        {
            return match version.parse() {
                Ok(version) if version > 0 => Ok(Self::Id {
                    id: id.to_owned(),
                    version: Some(version),
                }),
                _ => Err(wrong(format!(
                    "{raw}: the version after @ is a number from 1"
                ))),
            };
        }
        if is_id(raw) {
            return Ok(Self::Id {
                id: raw.to_owned(),
                version: None,
            });
        }
        if let Some((space, title)) = raw.split_once(':')
            && is_key(space)
            && !title.trim().is_empty()
        {
            return Ok(Self::Title {
                space: space.to_owned(),
                title: title.trim().to_owned(),
            });
        }
        Err(wrong(format!("{raw:?} is not a page")))
    }

    /// The page's id and the version asked for, looking a title or a
    /// space's homepage up.
    pub(crate) fn resolve(
        self,
        ctx: &Ctx,
        confluence: &Confluence,
    ) -> Result<(String, Option<u64>)> {
        match self {
            Self::Id { id, version } => Ok((id, version)),
            Self::Space(key) => {
                let space = confluence.space(ctx, &key)?;
                let home = text(&space["homepageId"]).ok_or_else(|| {
                    Failure::not_found(format!("space {key} has no homepage"))
                        .hint(format!("agent-cli confluence tree get {}", quote(&key)))
                })?;
                Ok((home, None))
            }
            Self::Title { space, title } => {
                let found = confluence.space(ctx, &space)?;
                let space_id = text(&found["id"]).unwrap_or_default();
                let url = confluence.v2(
                    "/pages",
                    &[
                        ("space-id", space_id),
                        ("title", title.clone()),
                        ("limit", "1".to_owned()),
                    ],
                );
                let answer = confluence.get(ctx, &url)?;
                let id = answer["results"]
                    .as_array()
                    .and_then(|pages| pages.first())
                    .and_then(|page| text(&page["id"]))
                    .ok_or_else(|| {
                        Failure::not_found(format!("no page titled {title:?} in space {space}"))
                            .hint(format!(
                                "agent-cli confluence page list {} --space {}",
                                quote(&title),
                                quote(&space)
                            ))
                    })?;
                Ok((id, None))
            }
        }
    }
}

/// A page ref resolved, with `--version` agreeing with any `@N` it carried.
pub(crate) fn page(
    ctx: &Ctx,
    confluence: &Confluence,
    raw: &str,
    version: Option<&str>,
) -> Result<(String, Option<u64>)> {
    let flag = match version {
        None => None,
        Some(flag) => Some(
            flag.trim()
                .parse::<u64>()
                .ok()
                .filter(|n| *n > 0)
                .ok_or_else(|| {
                    Failure::usage(format!("--version {flag} is not a version number"))
                })?,
        ),
    };
    let (id, given) = PageRef::parse(&confluence.site, raw)?.resolve(ctx, confluence)?;
    match (given, flag) {
        (Some(given), Some(flag)) if given != flag => Err(Failure::usage(format!(
            "{raw} names version {given}, and --version says {flag}"
        ))
        .hint(format!("agent-cli confluence page get {id}@{flag}"))
        .into()),
        (given, flag) => Ok((id, given.or(flag))),
    }
}

/// A page that must be its current version: writes take no `@N`.
pub(crate) fn current_page(ctx: &Ctx, confluence: &Confluence, raw: &str) -> Result<String> {
    match page(ctx, confluence, raw, None)? {
        (id, None) => Ok(id),
        (id, Some(version)) => Err(Failure::usage(format!(
            "{raw} names version {version}; only the current version can change"
        ))
        .hint(format!(
            "agent-cli confluence page get {id} --fields version"
        ))
        .into()),
    }
}

/// A space key, or the URL of anything in the space.
pub(crate) fn space_key(site: &Site, raw: &str) -> Result<String> {
    let raw = raw.trim();
    if let Some((path, _)) = web(site, raw)? {
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        if let ["spaces", key, ..] = segments[..] {
            return Ok(percent_decode(key));
        }
        return Err(Failure::usage(format!("{raw} is not a space URL"))
            .hint("agent-cli confluence space list")
            .into());
    }
    if is_key(raw) {
        return Ok(raw.to_owned());
    }
    Err(Failure::usage(format!("{raw:?} is not a space key"))
        .hint("agent-cli confluence space list")
        .into())
}

/// A comment's id, or a page URL with `focusedCommentId`.
pub(crate) fn comment(site: &Site, raw: &str) -> Result<String> {
    let raw = raw.trim();
    let wrong = |why: String| -> anyhow::Error {
        Failure::usage(why)
            .hint("agent-cli confluence comment list PAGE --fields id,kind,body")
            .into()
    };
    if let Some((_, query)) = web(site, raw)? {
        return param(&query, "focusedCommentId")
            .filter(|id| is_id(id))
            .ok_or_else(|| wrong(format!("{raw} names no comment (focusedCommentId)")));
    }
    if is_id(raw) {
        return Ok(raw.to_owned());
    }
    Err(wrong(format!("{raw:?} is not a comment id")))
}

/// An attachment as given.
#[derive(Debug, PartialEq)]
pub(crate) enum AttachmentRef {
    /// `att7001`
    Id(String),
    /// `/download/attachments/PAGE/NAME`: found by its file name.
    Named { page: String, name: String },
}

pub(crate) fn attachment(site: &Site, raw: &str) -> Result<AttachmentRef> {
    let raw = raw.trim();
    let wrong = |why: String| -> anyhow::Error {
        Failure::usage(why)
            .hint("agent-cli confluence attachment list PAGE --fields id,name")
            .into()
    };
    if let Some((path, query)) = web(site, raw)? {
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        // viewpageattachments.action?pageId=P&preview=/P/7001/name
        if let Some(preview) = param(&query, "preview") {
            let parts: Vec<&str> = preview.split('/').filter(|s| !s.is_empty()).collect();
            if let [_, id, ..] = parts[..]
                && is_id(id)
            {
                return Ok(AttachmentRef::Id(format!("att{id}")));
            }
        }
        if let ["download", "attachments", page, name, ..] = segments[..]
            && is_id(page)
        {
            return Ok(AttachmentRef::Named {
                page: page.to_owned(),
                name: percent_decode(name),
            });
        }
        return Err(wrong(format!("{raw} is not an attachment URL")));
    }
    let digits = raw.strip_prefix("att").unwrap_or(raw);
    if is_id(digits) {
        return Ok(AttachmentRef::Id(format!("att{digits}")));
    }
    Err(wrong(format!("{raw:?} is not an attachment id")))
}

/// The path (below `/wiki`) and query of a URL on the site; `None` when
/// `raw` is no URL. Another host is exit 2.
fn web(site: &Site, raw: &str) -> Result<Option<(String, String)>> {
    let Some(rest) = raw
        .strip_prefix("https://")
        .or_else(|| raw.strip_prefix("http://"))
    else {
        return Ok(None);
    };
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    if !host.eq_ignore_ascii_case(&site.host) {
        return Err(Failure::usage(format!(
            "{raw} is on {host}, and the configured Confluence site is {}",
            site.host
        ))
        .hint(format!("pass an id, or a URL on {}", site.web))
        .into());
    }
    let path = path.split('#').next().unwrap_or_default();
    let (path, query) = path.split_once('?').unwrap_or((path, ""));
    let path = format!("/{path}");
    if path.starts_with("/wiki/l/") || path.starts_with("/l/") {
        return Err(Failure::usage(format!(
            "{raw} is a share link, which only the browser can open"
        ))
        .hint("open it, and pass the page's own URL from the address bar")
        .into());
    }
    let path = path.strip_prefix("/wiki").unwrap_or(&path).to_owned();
    Ok(Some((path, query.to_owned())))
}

fn page_url(path: &str, query: &str) -> Option<PageRef> {
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let id = |id: &str, version: Option<u64>| {
        is_id(id).then(|| PageRef::Id {
            id: id.to_owned(),
            version,
        })
    };
    match segments[..] {
        ["spaces", _, "pages" | "blog", "edit-v2", page, ..] => id(page, None),
        ["spaces", _, "pages", page, ..] => id(page, None),
        ["spaces", _, "blog", _, _, _, page, ..] => id(page, None),
        ["spaces", key] | ["spaces", key, "overview"] => Some(PageRef::Space(percent_decode(key))),
        ["pages", "viewpage.action"] => id(
            &param(query, "pageId")?,
            param(query, "pageVersion").and_then(|v| v.parse().ok()),
        ),
        ["pages", "resumedraft.action"] => id(&param(query, "draftId")?, None),
        ["pages", "viewpageattachments.action"] => id(&param(query, "pageId")?, None),
        ["x", code] => tiny(code).map(|decoded| PageRef::Id {
            id: decoded.to_string(),
            version: None,
        }),
        ["display", key, title, ..] => Some(PageRef::Title {
            space: percent_decode(key),
            title: percent_decode(&title.replace('+', " ")),
        }),
        _ => None,
    }
}

/// A tiny link's code as the content id it stands for.
pub(crate) fn tiny(code: &str) -> Option<u64> {
    if code.is_empty() || code.len() > 11 {
        return None;
    }
    let mut bits: u128 = 0;
    for c in format!("{code:A<11}").chars() {
        let value = match c {
            'A'..='Z' => c as u8 - b'A',
            'a'..='z' => c as u8 - b'a' + 26,
            '0'..='9' => c as u8 - b'0' + 52,
            '_' | '+' => 62,
            '-' | '/' => 63,
            _ => return None,
        };
        bits = bits << 6 | u128::from(value);
    }
    // 66 bits: the 8 bytes, then two of padding.
    let bytes = ((bits >> 2) as u64).to_be_bytes();
    Some(u64::from_le_bytes(bytes))
}

fn param(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name)
            .then(|| percent_decode(value))
            .filter(|v| !v.is_empty())
    })
}

fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        match (bytes[at], bytes.get(at + 1), bytes.get(at + 2)) {
            (b'%', Some(&high), Some(&low)) if hex(high).is_some() && hex(low).is_some() => {
                out.push((hex(high).unwrap_or(0) * 16 + hex(low).unwrap_or(0)) as u8);
                at += 3;
            }
            (byte, _, _) => {
                out.push(byte);
                at += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn is_id(raw: &str) -> bool {
    !raw.is_empty() && raw.len() <= 20 && raw.bytes().all(|b| b.is_ascii_digit())
}

/// A space key: letters and digits, `~` for a personal space.
fn is_key(raw: &str) -> bool {
    !raw.is_empty() && raw.chars().all(|c| c.is_ascii_alphanumeric() || c == '~')
}

/// A word as a shell reads it back, for command lines in hints.
pub(crate) fn quote(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./:=,@%+~".contains(&b));
    if plain {
        word.to_owned()
    } else if word.contains('\'') {
        format!("\"{}\"", word.replace('"', "\\\""))
    } else {
        format!("'{word}'")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site() -> Site {
        Site::parse(Some("https://contoso.atlassian.net/wiki"), false).unwrap()
    }

    fn page(raw: &str) -> PageRef {
        PageRef::parse(&site(), raw).unwrap()
    }

    fn id(id: &str, version: Option<u64>) -> PageRef {
        PageRef::Id {
            id: id.to_owned(),
            version,
        }
    }

    #[test]
    fn tiny_links_decode_to_their_content_ids() {
        for (id, code) in [
            (123_456_789, "Fc1bBw"),
            (724_765_432, "_AozKw"),
            (98_765_432_101, "JeXg-hY"),
            (1101, "TQQ"),
        ] {
            assert_eq!(tiny(code), Some(id), "{code}");
        }
        assert_eq!(tiny("bad!"), None);
    }

    #[test]
    fn every_page_url_form_reads_as_the_page() {
        let base = "https://contoso.atlassian.net/wiki";
        for (url, want) in [
            (
                "/spaces/ENG/pages/1101/Runbook+etl_nightly",
                id("1101", None),
            ),
            ("/spaces/ENG/pages/1101", id("1101", None)),
            ("/spaces/ENG/pages/edit-v2/1101", id("1101", None)),
            ("/spaces/ENG/blog/2026/09/28/1401/Shipped", id("1401", None)),
            (
                "/pages/viewpage.action?pageId=1101&pageVersion=2",
                id("1101", Some(2)),
            ),
            ("/pages/resumedraft.action?draftId=1101", id("1101", None)),
            (
                "/spaces/ENG/pages/1201/Release?focusedCommentId=5002#comment-5002",
                id("1201", None),
            ),
            ("/x/TQQ", id("1101", None)),
            ("/spaces/ENG/overview", PageRef::Space("ENG".into())),
            (
                "/display/ENG/Runbook%3A+etl_nightly",
                PageRef::Title {
                    space: "ENG".into(),
                    title: "Runbook: etl_nightly".into(),
                },
            ),
        ] {
            assert_eq!(page(&format!("{base}{url}")), want, "{url}");
        }
        assert_eq!(page("1101@3"), id("1101", Some(3)));
        assert_eq!(
            page("ENG:Runbook: etl_nightly"),
            PageRef::Title {
                space: "ENG".into(),
                title: "Runbook: etl_nightly".into()
            }
        );
    }

    #[test]
    fn foreign_hosts_share_links_and_nonsense_are_exit_2() {
        for raw in [
            "https://fabrikam.atlassian.net/wiki/spaces/ENG/pages/1101",
            "https://contoso.atlassian.net/wiki/l/cp/AbCd",
            "https://contoso.atlassian.net/wiki/spaces/ENG/whiteboard/5",
            "1101@0",
            "runbook please",
        ] {
            let error = PageRef::parse(&site(), raw).unwrap_err();
            let failure = error.downcast_ref::<Failure>().unwrap();
            assert_eq!(failure.exit, agent_cli_core::Exit::Usage, "{raw}");
        }
    }

    #[test]
    fn comments_attachments_and_spaces_take_their_urls() {
        let site = site();
        assert_eq!(
            comment(
                &site,
                "https://contoso.atlassian.net/wiki/spaces/ENG/pages/1201/R?focusedCommentId=5002"
            )
            .unwrap(),
            "5002"
        );
        assert_eq!(
            attachment(&site, "7001").unwrap(),
            AttachmentRef::Id("att7001".into())
        );
        assert_eq!(
            attachment(&site, "https://contoso.atlassian.net/wiki/pages/viewpageattachments.action?pageId=1201&preview=%2F1201%2F7001%2Fchanges.txt").unwrap(),
            AttachmentRef::Id("att7001".into())
        );
        assert_eq!(
            attachment(&site, "https://contoso.atlassian.net/wiki/download/attachments/1201/rollout%20plan.png?api=v2").unwrap(),
            AttachmentRef::Named {
                page: "1201".into(),
                name: "rollout plan.png".into()
            }
        );
        assert_eq!(
            space_key(
                &site,
                "https://contoso.atlassian.net/wiki/spaces/ENG/pages/1101"
            )
            .unwrap(),
            "ENG"
        );
        assert_eq!(quote("Runbook: etl_nightly"), "'Runbook: etl_nightly'");
    }
}
