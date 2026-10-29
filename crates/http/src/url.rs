//! URLs as the domain handles them: checked, given query parameters, cut to
//! their origin, and joined with a `Location` or a docs page's relative link.
//! Parsed here, never fetched.

use agent_cli_core::{Failure, percent_encode};
use anyhow::Result;

/// `raw` when it is an `http://` or `https://` URL with a host; exit 2
/// otherwise, since curl's guess of a scheme is a guess an agent cannot see.
pub(crate) fn checked(raw: &str) -> Result<String> {
    let raw = raw.trim();
    let lower = raw.to_ascii_lowercase();
    let rest = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"));
    match rest {
        Some(rest)
            if !rest.is_empty() && !rest.starts_with('/') && !raw.contains(char::is_whitespace) =>
        {
            Ok(raw.to_owned())
        }
        _ => Err(
            Failure::usage(format!("{raw:?} is not an http:// or https:// URL"))
                .hint("give the whole URL, scheme included: https://api.contoso.example/orders")
                .into(),
        ),
    }
}

/// `url` with `pairs` added to its query, each `K=V` percent-encoded.
pub(crate) fn with_query(url: &str, pairs: &[String]) -> Result<String> {
    if pairs.is_empty() {
        return Ok(url.to_owned());
    }
    let (base, fragment) = match url.split_once('#') {
        Some((base, fragment)) => (base, Some(fragment)),
        None => (url, None),
    };
    let mut out = base.to_owned();
    for pair in pairs {
        let (key, value) = pair.split_once('=').ok_or_else(|| {
            Failure::usage(format!("--query {pair:?} is not K=V")).hint("--query status=open")
        })?;
        out.push(if out.contains('?') { '&' } else { '?' });
        percent_encode(key, &mut out);
        out.push('=');
        percent_encode(value, &mut out);
    }
    if let Some(fragment) = fragment {
        out.push('#');
        out.push_str(fragment);
    }
    Ok(out)
}

/// `scheme://host[:port]`, lowercased, the default port left out: what a
/// credential is held to.
pub(crate) fn origin(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_ascii_lowercase();
    };
    let scheme = scheme.to_ascii_lowercase();
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host)
        .to_ascii_lowercase();
    let default = if scheme == "https" { ":443" } else { ":80" };
    let authority = authority.strip_suffix(default).unwrap_or(&authority);
    format!("{scheme}://{authority}")
}

/// The path of `url`, without its query: `/orders/42`.
pub(crate) fn path(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let path = rest.find('/').map_or("/", |at| &rest[at..]);
    path.split(['?', '#']).next().unwrap_or("/").to_owned()
}

/// `reference` (a `Location`, a link in a page) read against `base`.
pub(crate) fn join(base: &str, reference: &str) -> String {
    let reference = reference.trim();
    if reference.contains("://") {
        return reference.to_owned();
    }
    let scheme = base.split_once("://").map_or("https", |(scheme, _)| scheme);
    if let Some(rest) = reference.strip_prefix("//") {
        return format!("{scheme}://{rest}");
    }
    let origin = origin(base);
    if reference.starts_with('/') {
        return format!("{origin}{reference}");
    }
    let base_path = path(base);
    if reference.is_empty() {
        return base.to_owned();
    }
    if reference.starts_with('?') {
        return format!("{origin}{base_path}{reference}");
    }
    let dir = base_path.rsplit_once('/').map_or("", |(dir, _)| dir);
    let mut segments: Vec<&str> = dir.split('/').filter(|s| !s.is_empty()).collect();
    let (relative, query) = match reference.split_once('?') {
        Some((relative, query)) => (relative, Some(query)),
        None => (reference, None),
    };
    for part in relative.split('/') {
        match part {
            "." | "" => {}
            ".." => {
                segments.pop();
            }
            part => segments.push(part),
        }
    }
    let trailing = if relative.ends_with('/') { "/" } else { "" };
    let mut joined = format!("{origin}/{}{trailing}", segments.join("/"));
    if let Some(query) = query {
        joined.push('?');
        joined.push_str(query);
    }
    joined
}

/// The host and port a URL or a `host[:port]` names, for a TLS probe.
pub(crate) fn host_and_port(raw: &str) -> Result<(String, u16)> {
    let raw = raw.trim();
    let (default, rest) = match raw.split_once("://") {
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("http") => (80, rest),
        Some((_, rest)) => (443, rest),
        None => (443, raw),
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let refused = || {
        Failure::usage(format!("{raw:?} is not HOST[:PORT] or a URL"))
            .hint("agent-cli http certificate get api.contoso.example:443")
    };
    let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
        let (host, after) = v6.split_once(']').ok_or_else(refused)?;
        (host.to_owned(), after.strip_prefix(':'))
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host.to_owned(), Some(port)),
            None => (authority.to_owned(), None),
        }
    };
    let port = match port {
        Some(port) => port.parse().map_err(|_| refused())?,
        None => default,
    };
    if host.is_empty() || host.contains(char::is_whitespace) {
        return Err(refused().into());
    }
    Ok((host.to_ascii_lowercase(), port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_url_needs_its_scheme_and_a_host() {
        assert!(checked("https://api.contoso.example/orders").is_ok());
        assert!(checked("http://localhost:8080").is_ok());
        assert!(checked("api.contoso.example/orders").is_err());
        assert!(checked("https:///orders").is_err());
        assert!(checked("ftp://files.contoso.example").is_err());
    }

    #[test]
    fn query_pairs_are_encoded_and_added_after_the_urls_own() {
        assert_eq!(
            with_query(
                "https://h.example/o?x=1",
                &["status=open now".into(), "a&b=c".into()]
            )
            .unwrap(),
            "https://h.example/o?x=1&status=open+now&a%26b=c"
        );
        assert!(with_query("https://h.example/", &["novalue".into()]).is_err());
    }

    #[test]
    fn an_origin_drops_the_path_the_user_and_the_default_port() {
        assert_eq!(
            origin("HTTPS://Api.Contoso.Example:443/x?y"),
            "https://api.contoso.example"
        );
        assert_eq!(
            origin("http://u:p@localhost:8080/x"),
            "http://localhost:8080"
        );
        assert_ne!(
            origin("https://a.example/"),
            origin("https://a.example:8443/")
        );
    }

    #[test]
    fn a_reference_joins_its_base_as_a_browser_joins_it() {
        let base = "https://h.example/swagger/index.html";
        assert_eq!(
            join(base, "v1/swagger.json"),
            "https://h.example/swagger/v1/swagger.json"
        );
        assert_eq!(
            join(base, "/openapi.json"),
            "https://h.example/openapi.json"
        );
        assert_eq!(
            join(base, "../api/doc.json"),
            "https://h.example/api/doc.json"
        );
        assert_eq!(join(base, "//other.example/x"), "https://other.example/x");
        assert_eq!(
            join(base, "https://x.example/s.json"),
            "https://x.example/s.json"
        );
        assert_eq!(
            join("https://h.example/api/schema/", "?format=json"),
            "https://h.example/api/schema/?format=json"
        );
        assert_eq!(
            join("https://h.example", "openapi.json"),
            "https://h.example/openapi.json"
        );
    }

    #[test]
    fn a_probe_target_is_a_host_and_port_from_whatever_was_given() {
        assert_eq!(
            host_and_port("api.contoso.example").unwrap(),
            ("api.contoso.example".into(), 443)
        );
        assert_eq!(
            host_and_port("https://localhost:58443/x").unwrap(),
            ("localhost".into(), 58443)
        );
        assert_eq!(host_and_port("[::1]:8443").unwrap(), ("::1".into(), 8443));
        assert!(host_and_port("host:notaport").is_err());
    }
}
