//! `aisearch document list`: `POST /indexes/{i}/docs/search`, always a POST
//! (vector queries are POST-only, and a URL is capped at 8 KB), with
//! `count: true` for the note's total.

use agent_cli_core::{Ctx, Failure, command};
use anyhow::Result;
use clap::builder::PossibleValuesParser;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::aisearch::document::{key_name, schema};
use crate::aisearch::index::named;
use crate::aisearch::shape::{Bounds, bound};
use crate::aisearch::{Role, holder, list, segment};
use crate::client::text;
use crate::config::Azure;

/// The service's caps on one request.
const TOP_MAX: usize = 1000;
const SKIP_MAX: usize = 100_000;

#[derive(clap::Args)]
pub struct DocumentListArgs {
    /// The index: SERVICE/INDEX from index list, a bare name or an alias
    index: String,
    /// What to search for (default: every document)
    text: Option<String>,
    /// The service that holds the index; needed when more than one does
    #[arg(long)]
    service: Option<String>,
    /// keyword (BM25), vector (TEXT embedded by the field's vectorizer), or hybrid (both, fused by RRF)
    #[arg(long, default_value = "keyword", value_parser = PossibleValuesParser::new(["keyword", "vector", "hybrid"]))]
    mode: String,
    /// Rerank keyword or hybrid results with the semantic ranker (top 50 only)
    #[arg(long)]
    semantic: bool,
    /// The semantic configuration (default: the index's)
    #[arg(long)]
    semantic_config: Option<String>,
    /// Full Lucene syntax (fields, fuzzy, regex); off, ? : / - " in TEXT stay plain text
    #[arg(long)]
    lucene: bool,
    /// Vector fields to query (default: every one with a vectorizer)
    #[arg(long)]
    vector_field: Vec<String>,
    /// An OData filter, such as "status eq 'failed' and total gt 100" (field names are case-sensitive)
    #[arg(long)]
    filter: Option<String>,
    /// Fields to return, comma-separated (default: every retrievable one)
    #[arg(long)]
    select: Option<String>,
    /// Sort, such as "created desc" (default: by score)
    #[arg(long)]
    orderby: Option<String>,
    /// Results to skip, for the next page (at most 100000)
    #[arg(long, default_value_t = 0)]
    skip: usize,
    /// Most rows to return (at most 1000)
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Hit {
    /// SERVICE/INDEX/KEY: what document get takes.
    id: Option<String>,
    /// BM25 for keyword, RRF for hybrid (small: about 0.03 is a strong match).
    score: Option<f64>,
    /// The semantic ranker's, 0 to 4, with --semantic.
    reranker: Option<f64>,
    caption: Option<String>,
    highlights: Option<Value>,
    /// The fields: strings cut at 200 characters, lists at 5 items, vectors
    /// as "[N floats]".
    doc: Value,
}

fn document_list(ctx: &Ctx, args: DocumentListArgs) -> Result<Vec<Hit>> {
    if args.limit > TOP_MAX || args.skip > SKIP_MAX {
        return Err(Failure::usage(format!(
            "--limit is at most {TOP_MAX} and --skip at most {SKIP_MAX}"
        ))
        .hint("narrow it with --filter, or sort with --orderby and filter past the last value seen")
        .into());
    }
    let vectors = args.mode != "keyword";
    if args.semantic && args.mode == "vector" {
        return Err(
            Failure::usage("--semantic reranks keyword or hybrid results")
                .hint("use --mode hybrid --semantic")
                .into(),
        );
    }
    if args.semantic && args.lucene {
        return Err(Failure::usage("--semantic and --lucene are two query types; pick one").into());
    }
    if args.lucene && args.mode == "vector" {
        return Err(
            Failure::usage("--lucene shapes keyword text; --mode vector sends none")
                .hint("use --mode hybrid --lucene")
                .into(),
        );
    }
    let text_given = args
        .text
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty());
    if vectors && text_given.is_none() {
        return Err(Failure::usage(format!("--mode {} needs TEXT to embed", args.mode)).into());
    }
    let azure = Azure::load(ctx)?;
    let (service, index) = named(&args.index, args.service.as_deref())?;
    let search = holder(ctx, &azure, "indexes", &index, service.as_deref())?;
    let schema = schema(ctx, &search, &index)?;
    let key = key_name(schema.as_ref());

    let mut body = json!({"count": true, "top": args.limit});
    if args.skip > 0 {
        body["skip"] = json!(args.skip);
    }
    if args.mode != "vector" {
        body["search"] = json!(text_given.unwrap_or("*"));
        body["queryType"] = json!(if args.lucene { "full" } else { "simple" });
    }
    if vectors {
        let fields = if args.vector_field.is_empty() {
            schema
                .iter()
                .flat_map(|schema| &schema.vectors)
                .filter(|(_, vectorized)| *vectorized)
                .map(|(name, _)| name.clone())
                .collect()
        } else {
            args.vector_field.clone()
        };
        if fields.is_empty() {
            return Err(Failure::usage(format!(
                "{}/{index} has no vector field with a vectorizer to embed TEXT",
                search.name()
            ))
            .hint("name one with --vector-field, or use --mode keyword")
            .into());
        }
        body["vectorQueries"] = json!([{"kind": "text", "text": text_given, "k": args.limit, "fields": fields.join(",")}]);
    }
    if args.semantic {
        let Some(config) = args
            .semantic_config
            .clone()
            .or_else(|| schema.as_ref().and_then(|schema| schema.semantic.clone()))
        else {
            return Err(Failure::usage(format!(
                "{}/{index} has no default semantic configuration",
                search.name()
            ))
            .hint(format!(
                "name one with --semantic-config; `agent-cli aisearch index get {}/{index} --fields semantic` lists them",
                search.name()
            ))
            .into());
        };
        body["queryType"] = json!("semantic");
        body["semanticConfiguration"] = json!(config);
        body["captions"] = json!("extractive");
        body["semanticErrorHandling"] = json!("partial");
    }
    for (flag, name) in [(&args.filter, "filter"), (&args.orderby, "orderby")] {
        if let Some(value) = flag {
            body[name] = json!(value);
        }
    }
    // Highlights say which words matched where: the answer to "why did this
    // come back".
    let searchable = schema.iter().flat_map(|schema| &schema.searchable);
    let searchable: Vec<&str> = searchable.map(String::as_str).collect();
    if text_given.is_some() && args.mode != "vector" && !searchable.is_empty() {
        body["highlight"] = json!(searchable.join(","));
    }
    if let Some(select) = &args.select {
        // The key builds each row's id, so it is always asked for.
        let asked: Vec<&str> = select.split(',').map(str::trim).collect();
        let mut select = asked.join(",");
        if !asked.contains(&key.as_str()) {
            select.push_str(&format!(",{key}"));
        }
        body["select"] = json!(select);
    }

    let response = search.query(
        &format!("/indexes/{}/docs/search", segment(&index)),
        body,
        Role::Query,
    )?;
    let answer = response.json()?;
    if response.status == 206 {
        ctx.note(format!(
            "[partial: semantic ranking {} ({}); --semantic again later, or without it]",
            text(&answer["@search.semanticPartialResponseReason"]).unwrap_or_default(),
            text(&answer["@search.semanticPartialResponseType"]).unwrap_or_default()
        ));
    }
    let mut cut = false;
    let hits: Vec<Hit> = list(&answer["value"])
        .iter()
        .map(|hit| {
            let doc: Map<String, Value> = hit
                .as_object()
                .into_iter()
                .flatten()
                .filter(|(name, _)| !name.starts_with("@search."))
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect();
            let id = doc
                .get(&key)
                .and_then(|held| text(held).or_else(|| Some(held.to_string())))
                .map(|held| format!("{}/{index}/{held}", search.name()));
            let caption = &hit["@search.captions"][0];
            Hit {
                id,
                score: hit["@search.score"].as_f64(),
                reranker: hit["@search.rerankerScore"].as_f64(),
                caption: text(&caption["text"]).or_else(|| text(&caption["highlights"])),
                highlights: Some(hit["@search.highlights"].clone()).filter(|h| !h.is_null()),
                doc: bound(
                    Value::Object(doc),
                    Bounds {
                        cut: true,
                        vectors: false,
                    },
                    &mut cut,
                ),
            }
        })
        .collect();
    if let Some(count) = answer["@odata.count"].as_u64()
        && count > (args.skip + hits.len()) as u64
    {
        ctx.note(format!(
            "[{} of {count}; --limit N or --skip {}]",
            hits.len(),
            args.skip + hits.len()
        ));
    }
    if cut && let Some(id) = hits.iter().find_map(|hit| hit.id.clone()) {
        ctx.note(format!(
            "[strings cut at 200 characters and lists at 5 items; one whole: agent-cli aisearch document get {id}]"
        ));
    }
    Ok(hits)
}

command! {
    pub DOCUMENT_LIST = ["aisearch", "document", "list"], Read,
    "Query an AI Search index: keyword, vector, hybrid or semantic search, filters",
    keywords: ["query", "find", "match", "retrieval", "rag", "results", "relevance", "odata", "rank"],
    example: "aisearch document list srch-contoso-prod/orders \"late delivery\" --filter \"status eq 'failed'\" --fields id,score,doc",
    run: document_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::AISEARCH;
    use crate::testing::{self, azure};

    fn definition() -> Answer {
        Answer::json(&json!({"name": "orders",
            "fields": [{"name": "id", "type": "Edm.String", "key": true}, {"name": "summary", "type": "Edm.String", "searchable": true},
                {"name": "summary_vector", "type": "Collection(Edm.Single)", "dimensions": 1536, "vectorSearchProfile": "orders-hnsw"}],
            "semantic": {"defaultConfiguration": "orders-semantic"},
            "vectorSearch": {"profiles": [{"name": "orders-hnsw", "algorithm": "hnsw", "vectorizer": "aoai-embed"}]}}))
    }

    fn one() -> Answer {
        testing::inventory(vec![testing::search_service(
            "srch-contoso-prod",
            "aadOrApiKey",
        )])
    }

    #[test]
    fn a_hybrid_semantic_search_sends_both_queries_and_bounds_what_comes_back() {
        let vector: Vec<f64> = (0..1536).map(|n| f64::from(n) / 10000.0).collect();
        let (outcome, transport) = azure(
            &[AISEARCH],
            &[
                "aisearch",
                "document",
                "list",
                "srch-contoso-prod/orders",
                "late delivery",
                "--mode",
                "hybrid",
                "--semantic",
                "--filter",
                "status eq 'failed'",
                "--select",
                "summary,status",
            ],
            vec![
                one(),
                definition(),
                Answer::json(&json!({"@odata.count": 3, "value": [
                    {"@search.score": 0.0331, "@search.rerankerScore": 2.71,
                     "@search.captions": [{"text": "Order 88122 arrived late.", "highlights": "<em>late</em>"}], "@search.highlights": {"summary": ["<em>late</em> delivery"]},
                     "id": "88122", "status": "failed", "summary": "y".repeat(300), "summary_vector": vector}]})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let hit = &outcome.json()[0];
        assert_eq!(hit["id"], "srch-contoso-prod/orders/88122");
        assert_eq!(hit["reranker"], 2.71);
        assert_eq!(hit["caption"], "Order 88122 arrived late.");
        assert_eq!(
            hit["highlights"],
            json!({"summary": ["<em>late</em> delivery"]})
        );
        assert_eq!(hit["doc"]["summary_vector"], "[1536 floats]");
        assert_eq!(hit["doc"]["summary"].as_str().unwrap().chars().count(), 201);
        assert!(
            outcome.stderr.contains("[1 of 3; --limit N or --skip 1]"),
            "{}",
            outcome.stderr
        );
        assert!(
            outcome.stderr.contains(
                "one whole: agent-cli aisearch document get srch-contoso-prod/orders/88122]"
            ),
            "{}",
            outcome.stderr
        );
        let sent = &transport.sent()[2];
        assert_eq!(
            sent.url,
            "https://srch-contoso-prod.search.windows.net/indexes/orders/docs/search?api-version=2026-04-01"
        );
        assert_eq!(
            sent.body.clone().unwrap(),
            json!({"count": true, "top": 50, "search": "late delivery", "queryType": "semantic",
                "vectorQueries": [{"kind": "text", "text": "late delivery", "k": 50, "fields": "summary_vector"}],
                "semanticConfiguration": "orders-semantic", "captions": "extractive", "semanticErrorHandling": "partial",
                "highlight": "summary", "filter": "status eq 'failed'", "select": "summary,status,id"})
        );
    }

    #[test]
    fn a_reader_who_cannot_read_the_definition_still_gets_rows_keyed_by_id() {
        let (outcome, transport) = azure(
            &[AISEARCH],
            &["aisearch", "document", "list", "srch-contoso-prod/orders"],
            vec![
                one(),
                Answer::status(
                    403,
                    r#"{"error":{"code":"","message":"Authorization failed."}}"#,
                ),
                testing::admin_keys(),
                Answer::status(
                    403,
                    r#"{"error":{"code":"","message":"Authorization failed."}}"#,
                ),
                Answer::json(&json!({"value": [{"@search.score": 1.0, "id": "88121"}]})),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json()[0]["id"], "srch-contoso-prod/orders/88121");
        assert!(
            outcome.stderr.contains("definition is not readable"),
            "{}",
            outcome.stderr
        );
        let body = transport.sent()[4].body.clone().unwrap();
        assert_eq!(
            body,
            json!({"count": true, "top": 50, "search": "*", "queryType": "simple"})
        );

        let (outcome, _) = azure(
            &[AISEARCH],
            &[
                "aisearch",
                "document",
                "list",
                "srch-contoso-prod/orders",
                "late",
                "--mode",
                "vector",
            ],
            vec![
                one(),
                Answer::status(
                    403,
                    r#"{"error":{"code":"","message":"Authorization failed."}}"#,
                ),
                testing::admin_keys(),
                Answer::status(
                    403,
                    r#"{"error":{"code":"","message":"Authorization failed."}}"#,
                ),
            ],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains("--vector-field"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn a_refused_token_with_no_key_to_fall_back_on_names_the_role() {
        let (outcome, _) = azure(
            &[AISEARCH],
            &[
                "aisearch",
                "document",
                "list",
                "srch-contoso-prod/orders",
                "--select",
                "id",
            ],
            vec![
                testing::inventory(vec![{
                    let mut row = testing::search_service("srch-contoso-prod", "");
                    row["disableLocalAuth"] = json!(true);
                    row
                }]),
                definition(),
                Answer::status(
                    403,
                    r#"{"error":{"code":"","message":"Authorization failed."}}"#,
                ),
            ],
        );
        assert_eq!(outcome.code, 3, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("needs Search Index Data Reader on srch-contoso-prod"),
            "{}",
            outcome.stderr
        );
        assert!(
            outcome.stderr.contains("agent-cli doctor aisearch"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn a_quota_429_fails_at_once_saying_quota_and_naming_the_usage_command() {
        let (outcome, transport) = azure(
            &[AISEARCH],
            &["aisearch", "document", "list", "srch-contoso-prod/orders"],
            vec![
                one(),
                definition(),
                Answer::status(
                    429,
                    r#"{"error":{"code":"","message":"You are running low on storage."}}"#,
                ),
            ],
        );
        assert_eq!(outcome.code, 1, "{outcome:?}");
        assert!(
            outcome.stderr.contains("the tier's quota, not a rate")
                && outcome
                    .stderr
                    .contains("agent-cli aisearch service get srch-contoso-prod"),
            "{}",
            outcome.stderr
        );
        let searches = transport
            .sent()
            .iter()
            .filter(|sent| sent.url.contains("/docs/search"))
            .count();
        assert_eq!(searches, 1, "a quota is not waited out and asked again");
    }
}
