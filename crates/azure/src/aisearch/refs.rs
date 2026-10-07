//! Ids as agents hand them over: `service/index`, `service/index/key`,
//! `service/indexer`, bare names with flags, data-plane URLs and portal
//! links. Read, never fetched.
//!
//! `/` is a safe separator: a document key holds only letters, digits, `-`,
//! `_` and `=`, and the service enforces it. Keys are carried verbatim, never
//! re-encoded: an indexer's are UrlTokenEncoded and end in a padding digit.

use agent_cli_core::Failure;
use anyhow::Result;

use crate::graph::SearchService;

/// What an id names. `service` is a name or a data-plane host.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Ref {
    pub service: Option<String>,
    pub index: Option<String>,
    pub key: Option<String>,
    pub indexer: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Want {
    Service,
    Index,
    Document,
    Indexer,
}

impl Want {
    fn forms(self) -> &'static str {
        match self {
            Self::Service => "SERVICE, its endpoint or its portal link",
            Self::Index => "INDEX, SERVICE/INDEX, its URL or its portal link",
            Self::Document => "SERVICE/INDEX/KEY, a KEY with --index, or its URL",
            Self::Indexer => "INDEXER, SERVICE/INDEXER, its URL or its portal link",
        }
    }
}

/// Reads `raw` as the thing `want` names.
pub(crate) fn parse(raw: &str, want: Want) -> Result<Ref> {
    let raw = raw.trim();
    let bad = || -> anyhow::Error {
        Failure::usage(format!("{raw:?} is not an id this takes"))
            .hint(format!("pass {}", want.forms()))
            .into()
    };
    let found = if let Some(rest) = raw.strip_prefix("https://") {
        let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
        let host = host.to_ascii_lowercase();
        if host.starts_with("portal.azure.") {
            portal(raw)
        } else if host.ends_with(".search.windows.net") {
            let mut found = data_plane(&rest[host.len()..]);
            found.service = Some(host);
            found
        } else {
            return Err(bad());
        }
    } else if raw.contains("('") {
        data_plane(raw)
    } else {
        let parts: Vec<String> = raw.split('/').map(str::to_owned).collect();
        if parts.iter().any(String::is_empty) {
            return Err(bad());
        }
        let mut parts = parts.into_iter();
        let (first, second, third) = (parts.next(), parts.next(), parts.next());
        if parts.next().is_some() {
            return Err(bad());
        }
        match (want, first, second, third) {
            (Want::Service, service, None, None) => Ref {
                service,
                ..Ref::default()
            },
            (Want::Index, index, None, None) => Ref {
                index,
                ..Ref::default()
            },
            (Want::Index, service, index @ Some(_), None) => Ref {
                service,
                index,
                ..Ref::default()
            },
            (Want::Document, key, None, None) => Ref {
                key,
                ..Ref::default()
            },
            (Want::Document, index, key @ Some(_), None) => Ref {
                index,
                key,
                ..Ref::default()
            },
            (Want::Document, service, index, key @ Some(_)) => Ref {
                service,
                index,
                key,
                ..Ref::default()
            },
            (Want::Indexer, indexer, None, None) => Ref {
                indexer,
                ..Ref::default()
            },
            (Want::Indexer, service, indexer @ Some(_), None) => Ref {
                service,
                indexer,
                ..Ref::default()
            },
            _ => return Err(bad()),
        }
    };
    let complete = match want {
        Want::Service => found.service.is_some(),
        Want::Index => found.index.is_some(),
        Want::Document => found.key.is_some(),
        Want::Indexer => found.indexer.is_some(),
    };
    if complete { Ok(found) } else { Err(bad()) }
}

/// `/indexes/orders/docs/88123`, `/indexes('orders')/docs('88123')`,
/// `/indexers/orders-sql/status`, with or without a query string.
fn data_plane(path: &str) -> Ref {
    let path = path.split(['?', '#']).next().unwrap_or_default();
    let mut segments: Vec<String> = Vec::new();
    for segment in path.split('/').filter(|segment| !segment.is_empty()) {
        match segment.split_once("('") {
            Some((name, quoted)) => {
                segments.push(name.to_owned());
                segments.push(decode(quoted.trim_end_matches("')")));
            }
            None => segments.push(decode(segment)),
        }
    }
    let mut found = Ref::default();
    let mut walk = segments.iter();
    while let Some(segment) = walk.next() {
        let next = || walk.clone().next().cloned();
        match segment.as_str() {
            "indexes" => found.index = next(),
            "indexers" => found.indexer = next(),
            "docs" => {
                found.key = next().filter(|key| {
                    !matches!(
                        key.as_str(),
                        "search" | "index" | "suggest" | "autocomplete" | "$count"
                    ) && !key.starts_with("search.")
                });
            }
            _ => continue,
        }
        walk.next();
    }
    found
}

/// A portal link: the fragment, percent-decoded once, holds the service's
/// ARM id, and `#name` after it names the object; the blade says what kind.
fn portal(raw: &str) -> Ref {
    let fragment = decode(raw.split_once('#').map_or("", |(_, fragment)| fragment));
    let lower = fragment.to_ascii_lowercase();
    let marker = "microsoft.search/searchservices/";
    let Some(at) = lower.find(marker) else {
        return Ref::default();
    };
    let rest = &fragment[at + marker.len()..];
    let end = rest.find(['/', '#', '?']).unwrap_or(rest.len());
    let mut found = Ref {
        service: Some(rest[..end].to_owned()).filter(|name| !name.is_empty()),
        ..Ref::default()
    };
    if let Some(object) = rest[end..].strip_prefix('#') {
        let object = object.split('/').next().unwrap_or_default().to_owned();
        if object.is_empty() {
            return found;
        }
        let blade = &lower[..at];
        if blade.contains("indexer") {
            found.indexer = Some(object);
        } else if blade.contains("index") {
            found.index = Some(object);
        }
    }
    found
}

/// One round of percent-decoding; a malformed escape stays as written.
fn decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let hex = |byte: u8| (byte as char).to_digit(16);
        if bytes[at] == b'%'
            && let (Some(high), Some(low)) = (
                bytes.get(at + 1).and_then(|b| hex(*b)),
                bytes.get(at + 2).and_then(|b| hex(*b)),
            )
        {
            out.push(u8::try_from(high * 16 + low).unwrap_or(b'?'));
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The service a name, a data-plane host, its endpoint or its portal link
/// stands for: a host is matched to a service's endpoint, else read as
/// `{name}.search.windows.net`.
pub(crate) fn service_name(found: &[SearchService], raw: &str) -> String {
    let named = parse(raw, Want::Service)
        .ok()
        .and_then(|found| found.service);
    let raw = named.as_deref().unwrap_or(raw).trim();
    if !raw.contains('.') {
        return raw.to_owned();
    }
    let host = raw.to_ascii_lowercase();
    found
        .iter()
        .find(|service| {
            let endpoint = service.endpoint.to_ascii_lowercase();
            let endpoint = endpoint.trim_start_matches("https://");
            endpoint.split('/').next() == Some(host.as_str())
        })
        .map(|service| service.name.clone())
        .unwrap_or_else(|| {
            host.strip_suffix(".search.windows.net")
                .and_then(|name| name.split('.').next())
                .map_or(raw.to_owned(), str::to_owned)
        })
}

/// The one value an id and a flag give, or exit 2 when they disagree.
pub(crate) fn agree(
    raw: &str,
    held: Option<String>,
    flag: Option<&str>,
    what: &str,
) -> Result<Option<String>> {
    match (held, flag) {
        (Some(held), Some(flag)) if !same(&held, flag) => Err(Failure::usage(format!(
            "{raw} names {what} {held}, and --{what} says {flag}"
        ))
        .into()),
        (held, flag) => Ok(held.or_else(|| flag.map(str::to_owned))),
    }
}

/// A service named once by its name and once by its host is the same one.
fn same(one: &str, other: &str) -> bool {
    let short = |name: &str| name.split('.').next().unwrap_or(name).to_ascii_lowercase();
    one.eq_ignore_ascii_case(other) || short(one) == short(other)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(raw: &str, want: Want) -> Ref {
        parse(raw, want).unwrap_or_else(|error| panic!("{raw}: {error:#}"))
    }

    fn of(service: Option<&str>, index: Option<&str>, key: Option<&str>) -> Ref {
        Ref {
            service: service.map(str::to_owned),
            index: index.map(str::to_owned),
            key: key.map(str::to_owned),
            indexer: None,
        }
    }

    #[test]
    fn an_index_is_its_id_a_bare_name_a_url_or_a_portal_link() {
        let prod = Some("srch-contoso-prod");
        assert_eq!(
            read("srch-contoso-prod/orders", Want::Index),
            of(prod, Some("orders"), None)
        );
        assert_eq!(read("orders", Want::Index), of(None, Some("orders"), None));
        assert_eq!(
            read(
                "https://srch-contoso-prod.search.windows.net/indexes/orders",
                Want::Index
            ),
            of(
                Some("srch-contoso-prod.search.windows.net"),
                Some("orders"),
                None
            )
        );
        assert_eq!(
            read(
                "https://SRCH-contoso-prod.search.windows.net/indexes('orders')?api-version=2026-04-01",
                Want::Index
            ),
            of(
                Some("srch-contoso-prod.search.windows.net"),
                Some("orders"),
                None
            )
        );
        assert_eq!(
            read("indexes('orders')", Want::Index),
            of(None, Some("orders"), None)
        );
        let link = "https://portal.azure.com/#view/Microsoft_Azure_Search/Index.ReactView/id/%2Fsubscriptions%2F00000000-0000-4000-8000-000000005b01%2FresourceGroups%2Frg-contoso-prod%2Fproviders%2FMicrosoft.Search%2FsearchServices%2Fsrch-contoso-prod%23orders/location/West%20Europe/sku/standard";
        assert_eq!(read(link, Want::Index), of(prod, Some("orders"), None));
        for bad in ["a/b/c", "", "/orders", "https://example.com/indexes/orders"] {
            let error = parse(bad, Want::Index).unwrap_err();
            let failure = error.downcast_ref::<Failure>().unwrap();
            assert_eq!(failure.exit, agent_cli_core::Exit::Usage, "{bad}");
        }
    }

    #[test]
    fn a_document_and_an_indexer_keep_their_keys_and_names_verbatim() {
        let prod = Some("srch-contoso-prod");
        assert_eq!(
            read("srch-contoso-prod/orders/88123", Want::Document),
            of(prod, Some("orders"), Some("88123"))
        );
        assert_eq!(
            read("aGVsbG8=1", Want::Document),
            of(None, None, Some("aGVsbG8=1"))
        );
        assert_eq!(
            read(
                "https://srch-contoso-prod.search.windows.net/indexes/orders/docs('88123')",
                Want::Document
            ),
            of(
                Some("srch-contoso-prod.search.windows.net"),
                Some("orders"),
                Some("88123")
            )
        );
        assert!(
            parse(
                "https://srch-contoso-prod.search.windows.net/indexes/orders/docs/search",
                Want::Document
            )
            .is_err()
        );
        let indexer = read(
            "https://srch-contoso-prod.search.windows.net/indexers/orders-sql/status",
            Want::Indexer,
        );
        assert_eq!(indexer.indexer.as_deref(), Some("orders-sql"));
        let link = "https://portal.azure.com/#view/Microsoft_Azure_Search/IndexerJsonEditor.ReactView/indexerId/%2Fsubscriptions%2Fs%2FresourceGroups%2Frg%2Fproviders%2FMicrosoft.Search%2FsearchServices%2Fsrch-contoso-prod%23orders-sql";
        let indexer = read(link, Want::Indexer);
        assert_eq!(
            (indexer.service.as_deref(), indexer.indexer.as_deref()),
            (prod, Some("orders-sql"))
        );
        assert!(
            parse(link, Want::Index).is_err(),
            "an indexer's link is no index"
        );
        let service = read(
            "https://portal.azure.com/#@contoso.onmicrosoft.com/resource/subscriptions/s/resourceGroups/rg/providers/Microsoft.Search/searchServices/srch-contoso-dev/overview",
            Want::Service,
        );
        assert_eq!(service.service.as_deref(), Some("srch-contoso-dev"));
    }

    #[test]
    fn a_host_is_matched_to_its_endpoint_and_a_flag_that_disagrees_is_refused() {
        let found = vec![SearchService {
            name: "srch-contoso-prod".into(),
            endpoint: "https://srch-contoso-prod-1a2b.sg.search.windows.net/".into(),
            ..SearchService::default()
        }];
        assert_eq!(
            service_name(&found, "srch-contoso-prod-1a2b.sg.search.windows.net"),
            "srch-contoso-prod"
        );
        assert_eq!(
            service_name(&found, "srch-contoso-dev.search.windows.net"),
            "srch-contoso-dev"
        );
        assert_eq!(
            service_name(
                &found,
                "https://srch-contoso-prod-1a2b.sg.search.windows.net/"
            ),
            "srch-contoso-prod"
        );
        assert_eq!(
            agree(
                "x",
                Some("srch-contoso-prod.search.windows.net".into()),
                Some("srch-contoso-prod"),
                "service"
            )
            .unwrap(),
            Some("srch-contoso-prod.search.windows.net".to_owned())
        );
        let error = agree(
            "x",
            Some("srch-contoso-prod".into()),
            Some("srch-contoso-dev"),
            "service",
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("--service says srch-contoso-dev"),
            "{error}"
        );
    }
}
