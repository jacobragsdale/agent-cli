//! `airflow instance`: the configured Airflow servers, and what a
//! `base_url` must look like.

pub(crate) mod list;

/// `https://host[:port][/prefix]`, or plain http to this machine (a compose
/// Airflow). Anything a URL parser might read two ways is refused.
pub(crate) fn check_base_url(raw: &str) -> Result<String, String> {
    let base = raw.trim().trim_end_matches('/');
    if let Some(server) = base.strip_suffix("/api/v2") {
        return Err(format!(
            "base_url {raw:?} ends in /api/v2; give the server without it: {server:?}"
        ));
    }
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
    use super::check_base_url;

    #[test]
    fn base_url_is_https_or_http_to_this_machine_and_never_ambiguous() {
        for (raw, want) in [
            (
                "https://airflow.contoso.example/",
                "https://airflow.contoso.example",
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
            "http://airflow.contoso.example",
            "airflow.contoso.example",
            "https://user@airflow.contoso.example",
            "https://airflow.contoso.example\\@evil",
            "https://airflow.contoso.example:",
            "https://airflow.contoso.example:80x",
            "https://airflow.contoso.example/x?y=1",
            "ftp://localhost",
            "http://localhost.evil.example",
        ] {
            assert!(check_base_url(raw).is_err(), "{raw}");
        }
        let api = check_base_url("https://airflow.contoso.example/api/v2/").unwrap_err();
        assert!(api.contains("\"https://airflow.contoso.example\""), "{api}");
    }
}
