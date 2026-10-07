//! `aisearch document`: what its verbs share. The index's definition gives
//! the key field (results don't mark it), the vector fields and the default
//! semantic configuration; the writes share one batch call.

pub(crate) mod create;
pub(crate) mod delete;
pub(crate) mod get;
pub(crate) mod list;
pub(crate) mod update;

use std::path::Path;

use agent_cli_core::{Ctx, Effect, Exit, Failure, Method};
use anyhow::Result;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::aisearch::index::key_field;
use crate::aisearch::refs::{Want, agree, parse};
use crate::aisearch::{Role, Search, list, segment};
use crate::client::{refused_with, text};
use crate::config::Azure;

/// A batch's largest input: the service's request cap.
const DOCS_MAX: usize = 16 * 1024 * 1024;
/// The service's cap on one batch.
const BATCH_MAX: usize = 1000;

/// What a document command needs of an index's definition. It holds field
/// names only, no secrets, so it may be cached.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct Schema {
    pub key: Option<String>,
    /// Vector fields, and whether each has a vectorizer (text queries need one).
    pub vectors: Vec<(String, bool)>,
    pub semantic: Option<String>,
}

/// The index's schema, read once per `[azure] refresh`; `None`, with a note
/// naming the role that reads it, when the login may only query.
pub(crate) fn schema(ctx: &Ctx, search: &Search<'_>, index: &str) -> Result<Option<Schema>> {
    let key = format!("azure:aisearch:schema:{}/{index}", search.name());
    if let Some(held) = ctx.cache().get::<Schema>(&key) {
        return Ok(Some(held));
    }
    let definition = match search.get(&format!("/indexes/{}", segment(index)), Role::Definitions) {
        Ok(definition) => definition,
        Err(error)
            if error.chain().any(|cause| {
                cause
                    .downcast_ref::<Failure>()
                    .is_some_and(|failure| failure.exit == Exit::Setup)
            }) || matches!(refused_with(&error), Some(401 | 403)) =>
        {
            ctx.note(format!(
                "[{}/{index}'s definition is not readable (Reader or Search Service Contributor reads it): rows use a field named id; vector modes need --vector-field]",
                search.name()
            ));
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    let profiles = list(&definition["vectorSearch"]["profiles"]);
    let schema = Schema {
        key: key_field(&definition),
        vectors: list(&definition["fields"])
            .iter()
            .filter_map(|field| {
                let profile = text(&field["vectorSearchProfile"])?;
                let vectorized = profiles.iter().any(|held| {
                    held["name"] == profile.as_str() && text(&held["vectorizer"]).is_some()
                });
                Some((text(&field["name"])?, vectorized))
            })
            .collect(),
        semantic: text(&definition["semantic"]["defaultConfiguration"]),
    };
    ctx.cache().put(&key, &schema, Azure::load(ctx)?.refresh());
    Ok(Some(schema))
}

/// The service, index and key a document id and the flags name together.
pub(crate) fn located(
    raw: &str,
    index: Option<&str>,
    service: Option<&str>,
) -> Result<(Option<String>, String, String)> {
    let found = parse(raw, Want::Document)?;
    let (flag_service, flag_index) = match index {
        Some(index) => {
            let flag = parse(index, Want::Index)?;
            (agree(index, flag.service, service, "service")?, flag.index)
        }
        None => (service.map(str::to_owned), None),
    };
    let service = agree(raw, found.service, flag_service.as_deref(), "service")?;
    let Some(index) = agree(raw, found.index, flag_index.as_deref(), "index")? else {
        return Err(Failure::usage(format!("{raw} names no index"))
            .hint("pass SERVICE/INDEX/KEY, or the key with --index SERVICE/INDEX")
            .into());
    };
    Ok((service, index, found.key.unwrap_or_default()))
}

/// The key field to read keys from: the schema's, else a field named `id`.
pub(crate) fn key_name(schema: Option<&Schema>) -> String {
    schema
        .and_then(|schema| schema.key.clone())
        .unwrap_or_else(|| "id".to_owned())
}

/// One item of a batch's answer.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Indexed {
    /// SERVICE/INDEX/KEY: what document get takes.
    id: String,
    key: String,
    /// The item's own HTTP status: 200 or 201 done; 409, 422 and 503 may be
    /// retried; 400 and 404 may not.
    status: Option<u64>,
    error: Option<String>,
}

/// The documents `--docs` or `--docs-file` holds: a JSON array, one object,
/// `{"value": […]}`, or JSON lines.
pub(crate) fn documents(ctx: &Ctx, value: Option<&str>, file: Option<&Path>) -> Result<Vec<Value>> {
    let Some(given) = ctx.long_text("docs", value, file, Some(DOCS_MAX))? else {
        return Err(Failure::usage("the documents are missing")
            .hint("pass --docs JSON, --docs - (stdin), or --docs-file FILE: a JSON array or JSON lines")
            .into());
    };
    let items = match serde_json::from_str::<Value>(&given.text) {
        Ok(Value::Array(items)) => items,
        Ok(Value::Object(mut object)) => match object.remove("value") {
            Some(Value::Array(items)) => items,
            _ => vec![Value::Object(object)],
        },
        Ok(_) => return Err(Failure::usage("the documents are not JSON objects").into()),
        Err(_) => given
            .text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                serde_json::from_str::<Value>(line).map_err(|error| {
                    Failure::usage(format!(
                        "the documents are neither JSON nor JSON lines: {error}"
                    ))
                    .into()
                })
            })
            .collect::<Result<Vec<_>>>()?,
    };
    if items.iter().any(|item| !item.is_object()) {
        return Err(Failure::usage("every document must be a JSON object").into());
    }
    if items.is_empty() || items.len() > BATCH_MAX {
        return Err(Failure::usage(format!(
            "a batch holds 1 to {BATCH_MAX} documents, and this has {}",
            items.len()
        ))
        .hint("split it, and send each part with its own command")
        .into());
    }
    Ok(items)
}

/// One `POST /indexes/{i}/docs/index` with every item's `@search.action` set.
/// A 207 (some items failed) is exit 1 with every item as data.
pub(crate) fn batch(
    search: &Search<'_>,
    index: &str,
    effect: Effect,
    action: &str,
    items: Vec<Value>,
) -> Result<Vec<Indexed>> {
    let sent = items.len();
    let value: Vec<Value> = items
        .into_iter()
        .map(|mut item| {
            item["@search.action"] = json!(action);
            item
        })
        .collect();
    let response = search.change(
        effect,
        Method::Post,
        &format!("/indexes/{}/docs/index", segment(index)),
        Some(json!({ "value": value })),
        &[],
        Role::Documents,
    )?;
    let answer = response.json()?;
    let rows: Vec<Indexed> = list(&answer["value"])
        .iter()
        .map(|item| {
            let key = text(&item["key"]).unwrap_or_default();
            Indexed {
                id: format!("{}/{index}/{key}", search.name()),
                key,
                status: item["statusCode"].as_u64(),
                error: text(&item["errorMessage"]),
            }
        })
        .collect();
    let failed = rows
        .iter()
        .filter(|row| row.status.is_some_and(|status| status >= 300))
        .count();
    if response.status == 207 || failed > 0 {
        return Err(Failure::new(
            Exit::Failed,
            format!("{failed} of {sent} documents failed in {}/{index}", search.name()),
        )
        .hint("items that failed with 409, 422 or 503 may be sent again; 400 and 404 need the document fixed")
        .with_data(&rows)
        .into());
    }
    Ok(rows)
}
