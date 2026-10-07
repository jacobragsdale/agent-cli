//! `airflow instance`: the configured Airflow servers, and what a
//! `base_url` must look like.

pub(crate) mod list;

/// Core's `check_base_url`, refusing the `/api/v1` or `/api/v2` agents paste
/// with the server.
pub(crate) fn check_base_url(raw: &str) -> Result<String, String> {
    let base = raw.trim().trim_end_matches('/');
    for api in ["/api/v1", "/api/v2"] {
        if let Some(server) = base.strip_suffix(api) {
            return Err(format!(
                "base_url {raw:?} ends in {api}; give the server without it: {server:?}"
            ));
        }
    }
    agent_cli_core::check_base_url(raw)
}

#[cfg(test)]
mod tests {
    use super::check_base_url;

    #[test]
    fn a_base_url_ending_in_the_api_path_names_the_server_without_it() {
        for api in ["/api/v2/", "/api/v1"] {
            let api = check_base_url(&format!("https://airflow.contoso.example{api}")).unwrap_err();
            assert!(api.contains("\"https://airflow.contoso.example\""), "{api}");
        }
    }
}
