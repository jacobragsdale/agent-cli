//! Finding a spec from what an agent was handed: the spec's own URL, a docs
//! page (Swagger UI, ReDoc, Scalar, RapiDoc, Stoplight Elements), or a base
//! URL, from which the paths frameworks serve their spec at are tried.
//!
//! The paths and the pages' ways of naming their spec are what the
//! frameworks were seen to serve (`scripts/openapi/FINDINGS.md`).

use std::time::Duration;

use agent_cli_core::{Body, Ctx, Failure, Method};
use anyhow::Result;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::call::Prepared;
use crate::url;

/// The most fetches a search may make.
const BUDGET: usize = 60;
/// How many of [`WELL_KNOWN`]'s paths the live harness confirms.
const CONFIRMED: usize = 8;
/// How long a base URL's answer is kept.
const KEEP: Duration = Duration::from_secs(3600);

/// A framework's default spec path and its docs page, most common first.
/// The first eight are held by the live harness; the rest come from the
/// frameworks' docs.
pub(crate) const WELL_KNOWN: &[(&str, &str, &str)] = &[
    ("/openapi.json", "/docs", "FastAPI"),
    (
        "/swagger/v1/swagger.json",
        "/swagger/index.html",
        "ASP.NET Core (Swashbuckle)",
    ),
    (
        "/openapi/v1.json",
        "/scalar/v1",
        "ASP.NET Core 9+ (MapOpenApi)",
    ),
    (
        "/v3/api-docs",
        "/swagger-ui/index.html",
        "Spring Boot (springdoc)",
    ),
    ("/api-json", "/api", "NestJS"),
    ("/swagger/doc.json", "/swagger/index.html", "Go (swag)"),
    (
        "/api/schema/",
        "/api/schema/swagger-ui/",
        "Django REST Framework (drf-spectacular)",
    ),
    ("/q/openapi", "/q/swagger-ui/", "Quarkus"),
    (
        "/swagger.json",
        "/swagger/",
        "drf-yasg, Flask-RESTX or go-swagger",
    ),
    ("/v2/api-docs", "/swagger-ui.html", "Spring (springfox)"),
    ("/api/openapi.json", "/api/docs", "Django Ninja"),
    ("/schema/openapi.json", "/schema", "Litestar"),
    ("/documentation/json", "/documentation", "Fastify"),
    ("/api-docs/openapi.json", "/swagger-ui", "utoipa"),
    ("/api/swagger.json", "/api/swagger/ui", "Azure Functions"),
    (
        "/swagger/docs/v1",
        "/swagger/ui/index",
        "ASP.NET (Swashbuckle 5)",
    ),
    ("/docs-json", "/docs", "NestJS"),
    ("/openapi.yaml", "", ""),
    ("/openapi", "", ""),
];

/// Where a spec was found, kept for an hour by what was given.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub(crate) struct Found {
    /// The spec's URL.
    pub(crate) url: String,
    /// `spec` (the URL given), `docs page` or `probed` (a well-known path).
    pub(crate) how: String,
    /// The docs page that shows it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) docs: Option<String>,
    /// The framework its path is the default of.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) framework: Option<String>,
    /// The fetches finding it took.
    pub(crate) fetches: usize,
    /// Other specs the docs page lists (versions, groups).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) others: Vec<Listed>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
pub(crate) struct Listed {
    pub(crate) name: String,
    pub(crate) url: String,
}

struct Search<'p, 'a> {
    ctx: &'a Ctx,
    prepared: &'p Prepared<'a>,
    fetches: usize,
    tried: Vec<String>,
}

/// A page's answer: status, text, `Content-Type`, the URL it came from after
/// redirects.
struct Page {
    status: u16,
    text: String,
    kind: String,
    url: String,
}

impl Search<'_, '_> {
    fn get(&mut self, address: &str) -> Result<Option<Page>> {
        if self.fetches >= BUDGET || self.tried.iter().any(|seen| seen == address) {
            return Ok(None);
        }
        self.fetches += 1;
        self.tried.push(address.to_owned());
        let exchange = self
            .prepared
            .send(self.ctx, Method::Get, address, Body::None, true)?;
        self.fetches += exchange.hops.len();
        let kind = exchange
            .response
            .header("content-type")
            .unwrap_or_default()
            .to_ascii_lowercase();
        Ok(Some(Page {
            status: exchange.response.status,
            text: exchange.response.body,
            kind,
            url: exchange.url,
        }))
    }

    /// The page at `address` when it is a spec.
    fn spec_at(&mut self, address: &str) -> Result<Option<Page>> {
        match self.get(address) {
            Ok(Some(page)) if page.status < 300 && is_spec(&page.text) => Ok(Some(page)),
            Ok(_) => Ok(None),
            // A well-known path that will not connect, after the first fetch
            // did, is a miss, not the end of the search.
            Err(error) if self.fetches > 1 => {
                let _ = error;
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    /// The specs a docs page names, the first that is one found.
    fn through_page(&mut self, page: &Page) -> Result<Option<(Page, Vec<Listed>)>> {
        let mut listed = links(&page.url, &page.text);
        let mut config_urls = configs(&page.url, &page.text);
        for script in scripts(&page.url, &page.text) {
            if let Some(code) = self.get(&script)?.filter(|code| code.status < 300) {
                listed.extend(links(&page.url, &code.text));
                config_urls.extend(configs(&page.url, &code.text));
            }
        }
        for config in config_urls {
            if let Some(answer) = self.get(&config)?.filter(|answer| answer.status < 300) {
                listed.extend(config_links(&config, &answer.text));
            }
        }
        // NestJS serves its page at PATH and its spec at PATH-json.
        let path = url::path(&page.url);
        let path = path.trim_end_matches('/');
        if !path.is_empty() && !path.ends_with(".html") {
            listed.push(Listed {
                name: "json".to_owned(),
                url: format!("{}{path}-json", url::origin(&page.url)),
            });
        }
        let mut unique: Vec<Listed> = Vec::new();
        for link in listed {
            if !unique.iter().any(|seen| seen.url == link.url) {
                unique.push(link);
            }
        }
        for (at, link) in unique.iter().enumerate() {
            if let Some(spec) = self.spec_at(&link.url)? {
                let others = unique
                    .iter()
                    .enumerate()
                    .filter(|(other, l)| *other != at && l.name != "json")
                    .map(|(_, l)| l.clone())
                    .collect();
                return Ok(Some((spec, others)));
            }
        }
        Ok(None)
    }
}

/// The spec `given` leads to, and its text.
pub(crate) fn find(ctx: &Ctx, prepared: &Prepared<'_>, given: &str) -> Result<(Found, String)> {
    let given = prepared.url(given)?;
    let key = format!("http-spec-at:{given}");
    let mut search = Search {
        ctx,
        prepared,
        fetches: 0,
        tried: Vec::new(),
    };
    if let Some(found) = ctx.cache().get::<Found>(&key)
        && let Some(page) = search.spec_at(&found.url)?
    {
        return Ok((found, page.text));
    }
    let found = discover(&mut search, &given)?;
    ctx.cache().put(&key, &found.0, KEEP);
    Ok(found)
}

fn discover(search: &mut Search<'_, '_>, given: &str) -> Result<(Found, String)> {
    let first = search.get(given)?;
    if let Some(page) = &first {
        if page.status < 300 && is_spec(&page.text) {
            let found = Found {
                url: page.url.clone(),
                how: "spec".to_owned(),
                docs: None,
                framework: framework_of(&page.url),
                fetches: search.fetches,
                others: Vec::new(),
            };
            return Ok((found, page.text.clone()));
        }
        if page.status < 300
            && is_html(page)
            && let Some((spec, others)) = search.through_page(page)?
        {
            let found = Found {
                framework: framework_of(&spec.url),
                url: spec.url,
                how: "docs page".to_owned(),
                docs: Some(page.url.clone()),
                fetches: search.fetches,
                others,
            };
            return Ok((found, spec.text));
        }
    }
    let origin = url::origin(given);
    let path = url::path(given);
    let own = path.trim_end_matches('/').to_owned();
    let prefixes: Vec<String> = if own.is_empty() {
        vec![String::new()]
    } else {
        vec![own, String::new()]
    };
    // The paths the harness holds under each prefix first, then the rest.
    let (held, rest) = WELL_KNOWN.split_at(CONFIRMED);
    for paths in [held, rest] {
        for prefix in &prefixes {
            for (spec_path, docs_path, framework) in paths {
                let address = format!("{origin}{prefix}{spec_path}");
                if let Some(page) = search.spec_at(&address)? {
                    let docs = if docs_path.is_empty() {
                        None
                    } else {
                        match search.get(&format!("{origin}{prefix}{docs_path}")) {
                            Ok(Some(answer)) if answer.status < 300 && is_html(&answer) => {
                                Some(answer.url)
                            }
                            _ => None,
                        }
                    };
                    let found = Found {
                        url: page.url.clone(),
                        how: "probed".to_owned(),
                        docs,
                        framework: (!framework.is_empty()).then(|| (*framework).to_owned()),
                        fetches: search.fetches,
                        others: Vec::new(),
                    };
                    return Ok((found, page.text));
                }
            }
        }
    }
    for prefix in &prefixes {
        let mut pages: Vec<&str> = WELL_KNOWN
            .iter()
            .map(|(_, docs, _)| *docs)
            .filter(|d| !d.is_empty())
            .collect();
        pages.dedup();
        for docs in pages {
            let address = format!("{origin}{prefix}{docs}");
            if let Ok(Some(page)) = search.get(&address)
                && page.status < 300
                && is_html(&page)
                && let Some((spec, others)) = search.through_page(&page)?
            {
                let found = Found {
                    framework: framework_of(&spec.url),
                    url: spec.url,
                    how: "docs page".to_owned(),
                    docs: Some(page.url),
                    fetches: search.fetches,
                    others,
                };
                return Ok((found, spec.text));
            }
        }
    }
    Err(Failure::not_found(format!(
        "no OpenAPI spec found from {given} in {} fetches: it is no spec, no docs page naming one, and none of the frameworks' paths answered with one",
        search.fetches
    ))
    .hint("give the spec's own URL, or a file: agent-cli http spec get ./openapi.yaml")
    .into())
}

/// Every spec `given` leads to: the one found, and those its docs page lists.
pub(crate) fn all(ctx: &Ctx, prepared: &Prepared<'_>, given: &str) -> Result<Vec<Listed>> {
    let (found, _) = find(ctx, prepared, given)?;
    let mut listed = vec![Listed {
        name: "default".to_owned(),
        url: found.url.clone(),
    }];
    let mut others = found.others.clone();
    if others.is_empty() {
        let mut search = Search {
            ctx,
            prepared,
            fetches: 0,
            tried: vec![found.url.clone()],
        };
        let docs = found.docs.clone().or_else(|| {
            let spec_path = url::path(&found.url);
            WELL_KNOWN
                .iter()
                .find(|(path, docs, _)| spec_path.ends_with(path) && !docs.is_empty())
                .map(|(path, docs, _)| {
                    let prefix = spec_path.strip_suffix(path).unwrap_or_default();
                    format!("{}{prefix}{docs}", url::origin(&found.url))
                })
        });
        if let Some(docs) = docs
            && let Ok(Some(page)) = search.get(&docs)
            && page.status < 300
            && is_html(&page)
        {
            let mut names = links(&page.url, &page.text);
            let mut config_urls = configs(&page.url, &page.text);
            for script in scripts(&page.url, &page.text) {
                if let Ok(Some(code)) = search.get(&script) {
                    names.extend(links(&page.url, &code.text));
                    config_urls.extend(configs(&page.url, &code.text));
                }
            }
            for config in config_urls {
                if let Ok(Some(answer)) = search.get(&config) {
                    names.extend(config_links(&config, &answer.text));
                }
            }
            others = names;
        }
    }
    for other in others {
        if !listed.iter().any(|seen| seen.url == other.url) {
            listed.push(other);
        }
    }
    Ok(listed)
}

fn framework_of(spec_url: &str) -> Option<String> {
    let path = url::path(spec_url);
    WELL_KNOWN
        .iter()
        .find(|(known, _, framework)| {
            !framework.is_empty() && path.ends_with(known.trim_end_matches('/'))
        })
        .map(|(_, _, framework)| (*framework).to_owned())
}

/// True for a JSON or YAML document with a top-level `openapi` or `swagger`.
pub(crate) fn is_spec(text: &str) -> bool {
    let trimmed = text.trim_start();
    if trimmed.starts_with('{') {
        return serde_json::from_str::<Value>(trimmed)
            .is_ok_and(|doc| doc["openapi"].is_string() || doc["swagger"].is_string());
    }
    trimmed
        .lines()
        .take(50)
        .any(|line| line.starts_with("openapi:") || line.starts_with("swagger:"))
}

fn is_html(page: &Page) -> bool {
    page.kind.contains("html") || page.text.trim_start().starts_with('<')
}

/// The keys docs pages name their spec by, as they write them.
const KEYS: [&str; 6] = [
    "url",
    "spec-url",
    "specUrl",
    "data-url",
    "apiDescriptionUrl",
    "Redoc.init(",
];

/// The spec URLs a page (or the script it loads) names, resolved as the
/// page would resolve them.
fn links(page_url: &str, text: &str) -> Vec<Listed> {
    // Scalar resolves its sources against the path before its own route,
    // not against the page.
    let base = match url::path(page_url).find("/scalar") {
        Some(at) => format!("{}{}/", url::origin(page_url), &url::path(page_url)[..at]),
        None => page_url.to_owned(),
    };
    let mut found = Vec::new();
    for key in KEYS {
        for value in values_of(text, key) {
            let value = value.replace("\\/", "/");
            let lower = value.to_ascii_lowercase();
            if value.is_empty()
                || value.starts_with('#')
                || lower.starts_with("javascript:")
                || lower.contains("petstore.swagger.io")
                || [".js", ".css", ".png", ".svg", ".ico", ".html", ".map"]
                    .iter()
                    .any(|end| lower.ends_with(end))
                || value.contains(['<', '>', '{', '$', ' '])
            {
                continue;
            }
            let address = url::join(&base, &value);
            if !found.iter().any(|seen: &Listed| seen.url == address) {
                found.push(Listed {
                    name: value,
                    url: address,
                });
            }
        }
    }
    found
}

/// The quoted values written after `key` and a `:` or `=`.
fn values_of(text: &str, key: &str) -> Vec<String> {
    let mut values = Vec::new();
    let bytes = text.as_bytes();
    let mut from = 0;
    while let Some(at) = text[from..].find(key) {
        let start = from + at;
        from = start + key.len();
        if key != "Redoc.init(" {
            let before = start.checked_sub(1).map(|i| bytes[i]);
            if before.is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
                continue;
            }
        }
        let mut i = from;
        let skip = |i: &mut usize| {
            while *i < bytes.len() && bytes[*i].is_ascii_whitespace() {
                *i += 1;
            }
        };
        if key != "Redoc.init(" {
            if i < bytes.len() && matches!(bytes[i], b'"' | b'\'') {
                i += 1;
            }
            skip(&mut i);
            if i >= bytes.len() || !matches!(bytes[i], b':' | b'=') {
                continue;
            }
            i += 1;
        }
        skip(&mut i);
        if i >= bytes.len() || !matches!(bytes[i], b'"' | b'\'' | b'`') {
            continue;
        }
        let quote = bytes[i];
        let begin = i + 1;
        let Some(len) = bytes[begin..].iter().position(|b| *b == quote) else {
            continue;
        };
        values.push(text[begin..begin + len].to_owned());
    }
    values
}

/// The scripts a Swagger UI page keeps its config in.
fn scripts(page_url: &str, text: &str) -> Vec<String> {
    values_of(text, "src")
        .into_iter()
        .filter(|src| {
            let name = src.rsplit('/').next().unwrap_or(src);
            matches!(
                name,
                "swagger-initializer.js" | "index.js" | "swagger-ui-init.js"
            )
        })
        .map(|src| url::join(page_url, &src))
        .take(3)
        .collect()
}

/// springdoc's `configUrl`, which lists its groups.
fn configs(page_url: &str, text: &str) -> Vec<String> {
    let mut found: Vec<String> = values_of(text, "configUrl")
        .into_iter()
        .map(|value| url::join(page_url, &value))
        .collect();
    found.dedup();
    found
}

/// A Swagger UI config's `urls` (or its one `url`).
fn config_links(config_url: &str, text: &str) -> Vec<Listed> {
    let Ok(config) = serde_json::from_str::<Value>(text) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    // springdoc serves every group together beside its config.
    if let Some(all) = config_url
        .split('?')
        .next()
        .and_then(|u| u.strip_suffix("/swagger-config"))
    {
        found.push(Listed {
            name: "all".to_owned(),
            url: all.to_owned(),
        });
    }
    for entry in config["urls"].as_array().into_iter().flatten() {
        if let Some(address) = entry["url"].as_str() {
            found.push(Listed {
                name: entry["name"].as_str().unwrap_or(address).to_owned(),
                url: url::join(config_url, address),
            });
        }
    }
    if let Some(address) = config["url"].as_str() {
        found.push(Listed {
            name: address.to_owned(),
            url: url::join(config_url, address),
        });
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn urls(found: Vec<Listed>) -> Vec<String> {
        found.into_iter().map(|l| l.url).collect()
    }

    #[test]
    fn each_frameworks_docs_page_names_its_spec_as_it_was_seen_to() {
        let fastapi = "<script src=\"https://cdn.jsdelivr.net/npm/swagger-ui-dist@5/swagger-ui-bundle.js\"></script>\n    const ui = SwaggerUIBundle({\n        url: '/openapi.json',";
        assert_eq!(
            urls(links("http://127.0.0.1:58080/docs", fastapi)),
            ["http://127.0.0.1:58080/openapi.json"]
        );
        let redoc = "<redoc spec-url=\"/openapi.json\"></redoc>";
        assert_eq!(
            urls(links("http://127.0.0.1:58080/redoc", redoc)),
            ["http://127.0.0.1:58080/openapi.json"]
        );
        let swashbuckle = r#"var configObject = JSON.parse('{"urls":[{"url":"v1/swagger.json","name":"Orders v1"}],"deepLinking":false}');"#;
        assert_eq!(
            urls(links(
                "http://127.0.0.1:58082/swagger/index.html",
                swashbuckle
            )),
            ["http://127.0.0.1:58082/swagger/v1/swagger.json"]
        );
        let scalar = r#"initialize('%2Fscalar%2F', false, {"favicon":"favicon.svg","_integration":"dotnet","sources":[{"title":"v1","url":"openapi/v1.json"}]},"#;
        assert_eq!(
            urls(links("http://127.0.0.1:58083/scalar/v1", scalar)),
            ["http://127.0.0.1:58083/openapi/v1.json"]
        );
        let springdoc = "window.ui = SwaggerUIBundle({\n  url: \"https://petstore.swagger.io/v2/swagger.json\",\n  \"configUrl\" : \"/v3/api-docs/swagger-config\",\n  \"validatorUrl\" : \"\"";
        assert!(links("http://127.0.0.1:58084/swagger-ui/index.html", springdoc).is_empty());
        assert_eq!(
            configs("http://127.0.0.1:58084/swagger-ui/index.html", springdoc),
            ["http://127.0.0.1:58084/v3/api-docs/swagger-config"]
        );
        let swag = "url: \"doc.json\",";
        assert_eq!(
            urls(links("http://127.0.0.1:58086/swagger/index.html", swag)),
            ["http://127.0.0.1:58086/swagger/doc.json"]
        );
        let page = "<script src=\"./swagger-initializer.js\"> </script><script src=\"index.js\" charset=\"utf-8\"></script>";
        assert_eq!(
            scripts("http://127.0.0.1:58086/swagger/index.html", page),
            [
                "http://127.0.0.1:58086/swagger/swagger-initializer.js",
                "http://127.0.0.1:58086/swagger/index.js",
            ]
        );
        let nest = "let url = window.location.search.match(/url=([^&]+)/);\n url = window.location.origin;";
        assert!(links("http://127.0.0.1:58085/api", nest).is_empty());
        let config = r#"{"configUrl":"/v3/api-docs/swagger-config","oauth2RedirectUrl":"http://127.0.0.1:58084/swagger-ui/oauth2-redirect.html","urls":[{"url":"/v3/api-docs/admin","name":"admin"},{"url":"/v3/api-docs/orders","name":"orders"}],"validatorUrl":""}"#;
        assert_eq!(
            config_links("http://127.0.0.1:58084/v3/api-docs/swagger-config", config),
            [
                Listed {
                    name: "all".into(),
                    url: "http://127.0.0.1:58084/v3/api-docs".into()
                },
                Listed {
                    name: "admin".into(),
                    url: "http://127.0.0.1:58084/v3/api-docs/admin".into()
                },
                Listed {
                    name: "orders".into(),
                    url: "http://127.0.0.1:58084/v3/api-docs/orders".into()
                },
            ]
        );
    }

    #[test]
    fn a_spec_is_json_or_yaml_with_openapi_or_swagger_at_the_top() {
        assert!(is_spec(r#"{"openapi": "3.1.0", "paths": {}}"#));
        assert!(is_spec(r#"{"swagger": "2.0"}"#));
        assert!(is_spec("openapi: 3.0.3\ninfo:\n  title: t\n"));
        assert!(!is_spec(r#"{"orders": "http://127.0.0.1:58087/orders/"}"#));
        assert!(!is_spec("<html>openapi: no</html>"));
    }
}
