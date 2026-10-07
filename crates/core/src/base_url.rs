//! A server the config names by its `base_url`, rather than a fixed cloud
//! host that `host_under` can check.

/// True when `url` is under `base` and a `/`: the token check for a server
/// the config names (`host_under` wants https with no port, and a compose
/// Airflow is `http://localhost:8080`). `base` is what [`check_base_url`]
/// returned.
#[must_use]
pub fn same_origin(base: &str, url: &str) -> bool {
    url.strip_prefix(base)
        .is_some_and(|rest| rest.starts_with('/'))
}

/// A configured server's `base_url`: `https://host[:port][/prefix]`, or plain
/// http to this machine, without its trailing slash. Anything a URL parser
/// might read two ways is refused.
pub fn check_base_url(raw: &str) -> Result<String, String> {
    let base = raw.trim().trim_end_matches('/');
    let wrong = || {
        format!(
            "base_url {raw:?} is not https://HOST[:PORT][/PREFIX] (plain http only to localhost)"
        )
    };
    let (scheme, rest) = base.split_once("://").ok_or_else(wrong)?;
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let odd_path = |c: char| !(c.is_ascii_alphanumeric() || "-._~/%".contains(c));
    let odd_host = |c: char| !(c.is_ascii_alphanumeric() || "-.:[]".contains(c));
    if authority.is_empty() || authority.contains(odd_host) || path.contains(odd_path) {
        return Err(wrong());
    }
    let host = match authority.split_once(']') {
        Some((v6, _)) => format!("{v6}]"),
        None => authority.split(':').next().unwrap_or_default().to_owned(),
    };
    let port = authority[host.len()..].strip_prefix(':');
    if port.is_some_and(|port| port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()))
        || !(authority.len() == host.len() || port.is_some())
    {
        return Err(wrong());
    }
    let local = matches!(host.as_str(), "localhost" | "127.0.0.1" | "[::1]");
    match scheme {
        "https" => Ok(base.to_owned()),
        "http" if local => Ok(base.to_owned()),
        _ => Err(wrong()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_is_https_or_http_to_this_machine_and_never_ambiguous() {
        for (raw, want) in [
            (
                "https://server.contoso.example/",
                "https://server.contoso.example",
            ),
            ("http://localhost:8080", "http://localhost:8080"),
            ("http://127.0.0.1:8080/", "http://127.0.0.1:8080"),
            ("http://[::1]:8080", "http://[::1]:8080"),
            (
                "https://contoso.example:8443/d-1a2b3c",
                "https://contoso.example:8443/d-1a2b3c",
            ),
        ] {
            assert_eq!(check_base_url(raw).as_deref(), Ok(want), "{raw}");
        }
        for raw in [
            "http://server.contoso.example",
            "server.contoso.example",
            "https://user@server.contoso.example",
            "https://server.contoso.example\\@evil",
            "https://server.contoso.example:",
            "https://server.contoso.example:80x",
            "https://server.contoso.example/x?y=1",
            "ftp://localhost",
            "http://localhost.evil.example",
        ] {
            assert!(check_base_url(raw).is_err(), "{raw}");
        }
    }

    #[test]
    fn a_token_goes_only_under_the_configured_base_url() {
        for (base, url) in [
            (
                "https://airflow.contoso.example",
                "https://airflow.contoso.example/api/v2/dags",
            ),
            ("http://localhost:8080", "http://localhost:8080/auth/token"),
            (
                "https://contoso.example/airflow",
                "https://contoso.example/airflow/api/v2/x",
            ),
        ] {
            assert!(same_origin(base, url), "{url}");
        }
        for (base, url) in [
            (
                "https://airflow.contoso.example",
                "https://airflow.contoso.example.evil.example/x",
            ),
            (
                "https://airflow.contoso.example",
                "https://airflow.contoso.example@evil.example/x",
            ),
            (
                "https://airflow.contoso.example",
                "http://airflow.contoso.example/x",
            ),
            (
                "https://airflow.contoso.example",
                "https://airflow.contoso.example:8443/x",
            ),
            (
                "https://contoso.example/airflow",
                "https://contoso.example/airflow2/x",
            ),
            (
                "https://contoso.example/airflow",
                "https://contoso.example/other",
            ),
            ("http://localhost:8080", "http://localhost:80801/x"),
        ] {
            assert!(!same_origin(base, url), "{url}");
        }
    }
}
