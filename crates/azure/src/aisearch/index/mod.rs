//! `aisearch index`: what its verbs share. An index definition is read into
//! a summary an agent can write a filter or pick a query mode from: which
//! fields filter, sort and facet, the vector fields and their vectorizers,
//! the semantic configurations.

pub(crate) mod create;
pub(crate) mod delete;
pub(crate) mod get;
pub(crate) mod list;
pub(crate) mod update;

use std::path::Path;

use agent_cli_core::{Ctx, Failure};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::aisearch::list;
use crate::aisearch::refs::{Want, agree, parse};
use crate::client::text;

/// A definition's largest JSON: the service's own request cap.
const DEFINITION_MAX: usize = 16 * 1024 * 1024;

/// The index an agent named, and the service it named with it.
pub(crate) fn named(raw: &str, service: Option<&str>) -> Result<(Option<String>, String)> {
    let found = parse(raw, Want::Index)?;
    let service = agree(raw, found.service, service, "service")?;
    Ok((service, found.index.unwrap_or_default()))
}

/// One index, summarized.
#[derive(Debug, Serialize, JsonSchema)]
pub struct IndexDetail {
    /// SERVICE/INDEX: what index get, document list and the writes take.
    id: String,
    service: String,
    name: String,
    documents: Option<u64>,
    /// Bytes.
    storage: Option<u64>,
    vector_storage: Option<u64>,
    /// The key field, which document get looks up.
    key: Option<String>,
    fields: Vec<Field>,
    semantic: Option<Semantic>,
    vector: Option<Vector>,
    scoring_profiles: Vec<String>,
    suggesters: Vec<Suggester>,
    /// What index update sends as If-Match.
    etag: Option<String>,
    /// With --full: the definition, every secret as "<unchanged>".
    pub(crate) definition: Option<Value>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Field {
    name: String,
    #[serde(rename = "type")]
    kind: Option<String>,
    /// Only the true ones of key, searchable, filterable, sortable,
    /// facetable, retrievable and stored.
    attrs: Vec<&'static str>,
    analyzer: Option<String>,
    dimensions: Option<u64>,
    /// The vector profile, which names the algorithm and the vectorizer.
    profile: Option<String>,
    /// A complex field's sub-fields.
    fields: Vec<Field>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Semantic {
    /// What document list --semantic uses without --semantic-config.
    default: Option<String>,
    configs: Vec<SemanticConfig>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SemanticConfig {
    name: String,
    title: Option<String>,
    content: Vec<String>,
    keywords: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Vector {
    profiles: Vec<Profile>,
    vectorizers: Vec<Vectorizer>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Profile {
    name: String,
    algorithm: Option<String>,
    vectorizer: Option<String>,
    compression: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Vectorizer {
    name: String,
    /// azureOpenAI, customWebApi, aml.
    kind: Option<String>,
    model: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Suggester {
    name: String,
    fields: Vec<String>,
}

const ATTRS: [&str; 7] = [
    "key",
    "searchable",
    "filterable",
    "sortable",
    "facetable",
    "retrievable",
    "stored",
];

fn field(raw: &Value) -> Field {
    Field {
        name: text(&raw["name"]).unwrap_or_default(),
        kind: text(&raw["type"]),
        attrs: ATTRS
            .into_iter()
            .filter(|attr| raw[*attr].as_bool() == Some(true))
            .collect(),
        analyzer: text(&raw["analyzer"]).or_else(|| text(&raw["indexAnalyzer"])),
        dimensions: raw["dimensions"].as_u64(),
        profile: text(&raw["vectorSearchProfile"]),
        fields: list(&raw["fields"]).iter().map(field).collect(),
    }
}

fn names(value: &Value, key: &str) -> Vec<String> {
    list(value)
        .iter()
        .filter_map(|item| text(&item[key]))
        .collect()
}

/// The summary of `definition`, with `stats` when they were read.
pub(crate) fn detail(service: &str, definition: &Value, stats: Option<&Value>) -> IndexDetail {
    let name = text(&definition["name"]).unwrap_or_default();
    let stat = |key: &str| stats.and_then(|stats| stats[key].as_u64());
    let semantic = &definition["semantic"];
    let vector = &definition["vectorSearch"];
    IndexDetail {
        id: format!("{service}/{name}"),
        service: service.to_owned(),
        documents: stat("documentCount"),
        storage: stat("storageSize"),
        vector_storage: stat("vectorIndexSize"),
        key: key_field(definition),
        fields: list(&definition["fields"]).iter().map(field).collect(),
        semantic: semantic.is_object().then(|| Semantic {
            default: text(&semantic["defaultConfiguration"]),
            configs: list(&semantic["configurations"])
                .iter()
                .map(|config| {
                    let fields = &config["prioritizedFields"];
                    SemanticConfig {
                        name: text(&config["name"]).unwrap_or_default(),
                        title: text(&fields["titleField"]["fieldName"]),
                        content: names(&fields["prioritizedContentFields"], "fieldName"),
                        keywords: names(&fields["prioritizedKeywordsFields"], "fieldName"),
                    }
                })
                .collect(),
        }),
        vector: vector.is_object().then(|| Vector {
            profiles: list(&vector["profiles"])
                .iter()
                .map(|profile| Profile {
                    name: text(&profile["name"]).unwrap_or_default(),
                    algorithm: text(&profile["algorithm"]),
                    vectorizer: text(&profile["vectorizer"]),
                    compression: text(&profile["compression"]),
                })
                .collect(),
            vectorizers: list(&vector["vectorizers"])
                .iter()
                .map(|vectorizer| {
                    let parameters = vectorizer
                        .as_object()
                        .into_iter()
                        .flatten()
                        .find(|(key, _)| key.ends_with("Parameters"))
                        .map_or(&Value::Null, |(_, parameters)| parameters);
                    Vectorizer {
                        name: text(&vectorizer["name"]).unwrap_or_default(),
                        kind: text(&vectorizer["kind"]),
                        model: text(&parameters["modelName"])
                            .or_else(|| text(&parameters["deploymentId"])),
                    }
                })
                .collect(),
        }),
        scoring_profiles: names(&definition["scoringProfiles"], "name"),
        suggesters: list(&definition["suggesters"])
            .iter()
            .map(|suggester| Suggester {
                name: text(&suggester["name"]).unwrap_or_default(),
                fields: list(&suggester["sourceFields"])
                    .iter()
                    .filter_map(text)
                    .collect(),
            })
            .collect(),
        etag: text(&definition["@odata.etag"]),
        definition: None,
        name,
    }
}

/// The definition's one top-level key field.
pub(crate) fn key_field(definition: &Value) -> Option<String> {
    list(&definition["fields"])
        .iter()
        .find(|field| field["key"].as_bool() == Some(true))
        .and_then(|field| text(&field["name"]))
}

/// The definition `--definition` or `--definition-file` holds, as an
/// object named `name` (its own `name`, when it has one, must agree).
pub(crate) fn definition(
    ctx: &Ctx,
    name: &str,
    value: Option<&str>,
    file: Option<&Path>,
) -> Result<Value> {
    let Some(given) = ctx.long_text("definition", value, file, Some(DEFINITION_MAX))? else {
        return Err(Failure::usage("the index definition is missing")
            .hint("pass --definition JSON, --definition - (stdin), or --definition-file FILE from `agent-cli aisearch index get ID --full --output FILE`")
            .into());
    };
    let mut definition: Value = serde_json::from_str(&given.text)
        .map_err(|error| Failure::usage(format!("the index definition is not JSON: {error}")))?;
    let Some(object) = definition.as_object_mut() else {
        return Err(Failure::usage("the index definition is not a JSON object").into());
    };
    object.remove("@odata.context");
    match object.get("name").and_then(Value::as_str) {
        Some(held) if held != name => {
            return Err(Failure::usage(format!(
                "the definition names index {held}, and the command says {name}"
            ))
            .into());
        }
        Some(_) => {}
        None => {
            object.insert("name".to_owned(), Value::String(name.to_owned()));
        }
    }
    Ok(definition)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_definition_is_summarized_without_false_noise() {
        let definition = json!({
            "name": "orders", "@odata.etag": "\"0x8DCE0A1B2C3D4E5\"",
            "fields": [
                {"name": "id", "type": "Edm.String", "key": true, "searchable": false, "retrievable": true},
                {"name": "summary_vector", "type": "Collection(Edm.Single)", "searchable": true, "dimensions": 1536, "vectorSearchProfile": "orders-hnsw"},
                {"name": "address", "type": "Edm.ComplexType", "fields": [{"name": "city", "type": "Edm.String", "filterable": true}]},
            ],
            "semantic": {"defaultConfiguration": "orders-semantic", "configurations": [{"name": "orders-semantic",
                "prioritizedFields": {"titleField": {"fieldName": "id"}, "prioritizedContentFields": [{"fieldName": "summary"}]}}]},
            "vectorSearch": {"profiles": [{"name": "orders-hnsw", "algorithm": "hnsw-1", "vectorizer": "aoai-embed"}],
                "vectorizers": [{"name": "aoai-embed", "kind": "azureOpenAI", "azureOpenAIParameters": {"deploymentId": "embed", "modelName": "text-embedding-3-small", "apiKey": "<redacted>"}}]},
        });
        let detail = serde_json::to_value(detail(
            "srch-contoso-prod",
            &definition,
            Some(&json!({"documentCount": 88120})),
        ))
        .unwrap();
        assert_eq!(detail["id"], "srch-contoso-prod/orders");
        assert_eq!(detail["key"], "id");
        assert_eq!(detail["documents"], 88120);
        assert_eq!(detail["fields"][0]["attrs"], json!(["key", "retrievable"]));
        assert_eq!(detail["fields"][1]["profile"], "orders-hnsw");
        assert_eq!(
            detail["fields"][2]["fields"][0]["attrs"],
            json!(["filterable"])
        );
        assert_eq!(
            detail["semantic"]["configs"][0]["content"],
            json!(["summary"])
        );
        assert_eq!(
            detail["vector"]["vectorizers"][0],
            json!({"name": "aoai-embed", "kind": "azureOpenAI", "model": "text-embedding-3-small"})
        );
        assert_eq!(detail["etag"], "\"0x8DCE0A1B2C3D4E5\"");
    }
}
