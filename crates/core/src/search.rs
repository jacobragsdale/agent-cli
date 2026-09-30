//! `agent-cli search`: the front door.
//!
//! A port of the api-cli ranker (`cli.py.tmpl` `rank`): BM25F over the path
//! (x3), summary (x2), keywords, arg names and return field names, times the
//! squared share of query words matched, with prefix matching, a light
//! stemmer and verb synonyms. On top of that, each domain's own synonyms
//! ("ticket" -> workitem), which count for that domain's commands only: ado's
//! "build" means `ado run`, not every `run`. A question ("which pods…",
//! "why did…") ranks changes below reads. At 1,000 in-memory commands it
//! needs no index.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::discover::{required_args, return_fields};
use crate::registry::{Command, Domain, Effect};

pub(crate) const STOP: &[&str] = &[
    "a", "an", "the", "to", "for", "of", "in", "on", "with", "from", "by", "and", "or", "is",
    "are", "be", "this", "that", "it", "i", "what", "which", "how", "do", "does", "can", "some",
    "any", "into", "at", "as", "am", "was", "were", "there", "did", "has", "have", "had", "been",
    "being", "not",
];

/// Words that ask to see something without saying what: they add their verb
/// synonyms' score but do not count toward the share of words matched, so
/// "show" matching `list` cannot outweigh the words that name the thing.
const FILLER: &[&str] = &["show", "view", "display", "see"];

/// A query starting with one of these asks to read something, so writes and
/// destructive commands rank at half their score. Not "how" ("how do I
/// restart…") and not "get" ("get kubeconfig credentials").
const QUESTIONS: &[&str] = &[
    "what", "which", "why", "who", "whose", "when", "where", "is", "are", "was", "were", "did",
    "does", "has", "have", "show", "list",
];

/// Verb synonyms, weighted half a direct hit.
const SYNONYMS: &[(&str, &[&str])] = &[
    ("remove", &["delete"]),
    ("delete", &["remove"]),
    ("add", &["create"]),
    ("new", &["create"]),
    ("create", &["add", "new"]),
    ("show", &["get", "list"]),
    ("view", &["get"]),
    ("fetch", &["get"]),
    ("read", &["get"]),
    ("find", &["search", "list", "get"]),
    ("search", &["find", "list"]),
    ("edit", &["update"]),
    ("update", &["edit", "set"]),
    ("change", &["update", "edit", "set"]),
    ("modify", &["update", "edit"]),
    ("close", &["update", "edit", "state"]),
    ("reopen", &["update", "edit", "state"]),
    ("rename", &["update", "edit"]),
    ("all", &["list"]),
    ("list", &["all", "get"]),
    ("my", &["authenticated", "current", "user"]),
    ("me", &["authenticated", "current", "user"]),
    ("mine", &["authenticated", "current", "user"]),
    ("who", &["authenticated", "user"]),
    ("watch", &["stream", "ws"]),
    ("stream", &["ws"]),
    ("live", &["stream", "ws"]),
    ("listen", &["stream", "ws"]),
    ("tail", &["stream", "ws", "logs"]),
    ("follow", &["stream", "ws", "logs"]),
    ("realtime", &["stream", "ws"]),
    // A time asks for a window, which every command filtering by time takes
    // as --since.
    ("today", &["since"]),
    ("yesterday", &["since"]),
    ("night", &["since"]),
    ("hour", &["since"]),
    ("hours", &["since"]),
    ("day", &["since"]),
    ("days", &["since"]),
    ("week", &["since"]),
    ("weeks", &["since"]),
    ("month", &["since"]),
    ("months", &["since"]),
    ("ago", &["since"]),
];

/// Path, summary, keywords, arg names, return field names.
const WEIGHTS: [f64; 5] = [3.0, 2.0, 1.0, 0.4, 0.4];

pub(crate) struct Hit<'a> {
    pub score: f64,
    /// The share of query words that matched something.
    pub coverage: f64,
    pub command: &'a Command,
}

struct Doc<'a> {
    command: &'a Command,
    fields: [HashMap<String, usize>; 5],
    lengths: [usize; 5],
}

/// Every command that matches any query word, best first.
pub(crate) fn rank<'a>(domains: &'a [Domain], query: &str) -> Vec<Hit<'a>> {
    let docs: Vec<Doc<'a>> = domains
        .iter()
        .flat_map(|domain| domain.commands)
        .map(doc)
        .collect();
    if docs.is_empty() {
        return Vec::new();
    }
    let n = docs.len() as f64;
    let mut average = [0.0; 5];
    for (field, slot) in average.iter_mut().enumerate() {
        let total: usize = docs.iter().map(|doc| doc.lengths[field]).sum();
        *slot = if total == 0 { 1.0 } else { total as f64 / n };
    }
    let mut df: HashMap<&str, usize> = HashMap::new();
    for doc in &docs {
        let terms: HashSet<&str> = doc
            .fields
            .iter()
            .flat_map(|field| field.keys().map(String::as_str))
            .collect();
        for term in terms {
            *df.entry(term).or_default() += 1;
        }
    }
    let by_domain: HashMap<&str, Vec<HashMap<&str, f64>>> = domains
        .iter()
        .map(|domain| (domain.name, expand(domain.synonyms, query, &df)))
        .collect();
    if by_domain.values().next().is_none_or(Vec::is_empty) {
        return Vec::new();
    }
    let question = split_words(query)
        .first()
        .is_some_and(|word| QUESTIONS.contains(&word.as_str()));
    let mut filler: Vec<bool> = terms(query)
        .iter()
        .map(|term| FILLER.contains(&term.as_str()))
        .collect();
    let mut counted = filler.iter().filter(|filler| !**filler).count();
    if counted == 0 {
        // Nothing but filler ("show"): every word counts after all.
        filler.iter_mut().for_each(|filler| *filler = false);
        counted = filler.len();
    }
    let mut hits: Vec<Hit<'a>> = Vec::new();
    for doc in &docs {
        let Some(expansions) = by_domain.get(doc.command.path[0]) else {
            continue;
        };
        let (mut total, mut matched) = (0.0, 0);
        for (slot, expansion) in expansions.iter().enumerate() {
            let mut best: f64 = 0.0;
            for (term, weight) in expansion {
                let tf: f64 = (0..5)
                    .filter_map(|field| {
                        let count = *doc.fields[field].get(*term)? as f64;
                        let norm = 0.5 + 0.5 * doc.lengths[field] as f64 / average[field];
                        Some(WEIGHTS[field] * count / norm)
                    })
                    .sum();
                if tf > 0.0 {
                    let seen = df[term] as f64;
                    let idf = (1.0 + (n - seen + 0.5) / (seen + 0.5)).ln();
                    best = best.max(weight * idf * tf / (tf + 1.2));
                }
            }
            if best > 0.0 {
                matched += usize::from(!filler[slot]);
                total += best;
            }
        }
        if total > 0.0 {
            let coverage = matched as f64 / counted as f64;
            let asks =
                question && matches!(doc.command.effect, Effect::Write | Effect::Destructive);
            hits.push(Hit {
                score: total * coverage * coverage * if asks { 0.5 } else { 1.0 },
                coverage,
                command: doc.command,
            });
        }
    }
    hits.sort_by(|a, b| b.score.total_cmp(&a.score));
    hits
}

/// Each query word as the indexed terms it can match, with a weight: itself
/// (1.0), one of `synonyms` (a domain's; 1.0), a verb synonym (0.5), and
/// prefixes of either (x0.7).
fn expand<'d>(
    synonyms: &[(&str, &[&str])],
    query: &str,
    df: &HashMap<&'d str, usize>,
) -> Vec<HashMap<&'d str, f64>> {
    let terms = terms(query);
    let mut candidates: Vec<Vec<(String, f64)>> = terms
        .iter()
        .map(|word| {
            let mut own = vec![(stem(word), 1.0)];
            if let Some((_, synonyms)) = SYNONYMS.iter().find(|(key, _)| key == word) {
                own.extend(synonyms.iter().map(|synonym| (stem(synonym), 0.5)));
            }
            own
        })
        .collect();
    for (key, targets) in synonyms {
        let key = split_words(key);
        if key.is_empty() || key.len() > terms.len() {
            continue;
        }
        for start in 0..=terms.len() - key.len() {
            if terms[start..start + key.len()] == key[..] {
                for slot in &mut candidates[start..start + key.len()] {
                    slot.extend(targets.iter().flat_map(|target| {
                        split_words(target)
                            .into_iter()
                            .map(|word| (stem(&word), 1.0))
                    }));
                }
            }
        }
    }
    candidates
        .into_iter()
        .map(|options| {
            let mut expansion: HashMap<&'d str, f64> = HashMap::new();
            for (wanted, weight) in options {
                for &term in df.keys() {
                    let score = if term == wanted {
                        weight
                    } else if (wanted.len() >= 3 && term.starts_with(&wanted))
                        || (term.len() >= 4 && wanted.starts_with(term))
                    {
                        weight * 0.7
                    } else {
                        continue;
                    };
                    let slot = expansion.entry(term).or_default();
                    *slot = slot.max(score);
                }
            }
            expansion
        })
        .collect()
}

/// The query's words that count: all but stop words, or all of them when
/// nothing else is left.
fn terms(query: &str) -> Vec<String> {
    let words = split_words(query);
    let terms: Vec<String> = words
        .iter()
        .filter(|word| !STOP.contains(&word.as_str()))
        .cloned()
        .collect();
    if terms.is_empty() { words } else { terms }
}

fn doc(command: &Command) -> Doc<'_> {
    let args = (command.args)();
    let arg_names: Vec<&str> = args
        .get_arguments()
        .map(|arg| arg.get_id().as_str())
        .collect();
    let texts = [
        command.path.join(" "),
        command.summary.to_owned(),
        command.keywords.join(" "),
        arg_names.join(" "),
        return_fields(&(command.returns)()).join(" "),
    ];
    let mut fields: [HashMap<String, usize>; 5] = Default::default();
    let mut lengths = [0; 5];
    for (index, text) in texts.iter().enumerate() {
        for word in split_words(text) {
            if STOP.contains(&word.as_str()) {
                continue;
            }
            *fields[index].entry(stem(&word)).or_default() += 1;
            lengths[index] += 1;
        }
    }
    Doc {
        command,
        fields,
        lengths,
    }
}

/// The lines `agent-cli search` prints. When the best hit matched fewer than
/// half the words, a first line says the hits are guesses.
pub(crate) fn search_lines(domains: &[Domain], query: &str, limit: usize) -> Vec<String> {
    let hits = rank(domains, query);
    let mut lines = Vec::new();
    if hits.first().is_some_and(|hit| hit.coverage < 0.5) {
        lines.push("(no command matches most of these words; closest:)".to_owned());
    }
    lines.extend(hits.iter().take(limit).map(|hit| hit_line(hit.command)));
    if hits.len() > limit {
        lines.push(format!(
            "({limit} of {}; --limit N for more. Details: agent-cli <domain> <resource> <verb> --help)",
            hits.len()
        ));
    }
    lines
}

pub(crate) fn hit_line(command: &Command) -> String {
    let path = command.path.join(" ");
    let required = required_args(command);
    let head = format!("agent-cli {path} {required}");
    format!(
        "{}  # {}{}",
        head.trim_end(),
        command.summary,
        command.effect.tag()
    )
}

/// A light stemmer: plurals and `-ing`/`-ed`, never below three letters.
fn stem(word: &str) -> String {
    for (suffix, replacement) in [
        ("ies", "y"),
        ("sses", "ss"),
        ("ing", ""),
        ("ed", ""),
        ("es", ""),
        ("s", ""),
    ] {
        if let Some(root) = word.strip_suffix(suffix)
            && root.len() >= 3
        {
            return format!("{root}{replacement}");
        }
    }
    word.to_owned()
}

/// Lowercase words, split on anything not alphanumeric and at camelCase humps.
fn split_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut previous: Option<char> = None;
    for c in text.chars() {
        if !c.is_alphanumeric() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
        } else {
            if c.is_uppercase()
                && previous.is_some_and(|p| p.is_lowercase() || p.is_ascii_digit())
                && !word.is_empty()
            {
                words.push(std::mem::take(&mut word));
            }
            word.extend(c.to_lowercase());
        }
        previous = Some(c);
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

/// How well search finds the command each labeled query names.
#[derive(Debug)]
pub struct Quality {
    pub total: usize,
    pub top1: usize,
    pub top5: usize,
    /// One line per query whose command was not first.
    pub misses: Vec<String>,
}

impl Quality {
    #[must_use]
    pub fn top1_rate(&self) -> f64 {
        rate(self.top1, self.total)
    }

    #[must_use]
    pub fn top5_rate(&self) -> f64 {
        rate(self.top5, self.total)
    }
}

fn rate(part: usize, total: usize) -> f64 {
    if total == 0 {
        1.0
    } else {
        part as f64 / total as f64
    }
}

#[derive(Deserialize)]
struct Labeled {
    #[serde(default)]
    query: Vec<Query>,
}

#[derive(Deserialize)]
struct Query {
    text: String,
    expect: String,
}

/// Runs every `[[query]] text = "…" expect = "domain resource verb"` in
/// `labeled` against the registry. An `expect` naming no command is an error,
/// not a miss.
pub fn quality(domains: &[Domain], labeled: &str) -> Result<Quality> {
    let file: Labeled =
        toml::from_str(labeled).context("the labeled queries are not valid TOML")?;
    let known: HashSet<String> = domains
        .iter()
        .flat_map(|domain| domain.commands)
        .map(|command| command.path.join(" "))
        .collect();
    let mut quality = Quality {
        total: 0,
        top1: 0,
        top5: 0,
        misses: Vec::new(),
    };
    for query in file.query {
        anyhow::ensure!(
            known.contains(&query.expect),
            "{:?} expects `{}`, which is not a command",
            query.text,
            query.expect
        );
        let ranked: Vec<String> = rank(domains, &query.text)
            .iter()
            .take(5)
            .map(|hit| hit.command.path.join(" "))
            .collect();
        let at = ranked.iter().position(|path| *path == query.expect);
        quality.total += 1;
        quality.top1 += usize::from(at == Some(0));
        quality.top5 += usize::from(at.is_some());
        if at != Some(0) {
            quality.misses.push(format!(
                "{:?}: want `{}`, got [{}]",
                query.text,
                query.expect,
                ranked
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    Ok(quality)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_split_on_humps_and_punctuation_and_stem_lightly() {
        assert_eq!(
            split_words("listWorkItems, PR-42!"),
            ["list", "work", "items", "pr", "42"]
        );
        assert_eq!(split_words("HTTPServer v2Api"), ["httpserver", "v2", "api"]);
        assert_eq!(stem("queries"), "query");
        assert_eq!(stem("running"), "runn");
        assert_eq!(stem("pods"), "pod");
        assert_eq!(stem("prs"), "prs", "never below three letters");
        assert_eq!(stem("classes"), "class");
    }

    #[test]
    fn an_empty_label_file_is_perfect_and_a_bad_expect_is_an_error() {
        let quality = quality(&[], "").unwrap();
        assert_eq!(
            (quality.total, quality.top1_rate(), quality.top5_rate()),
            (0, 1.0, 1.0)
        );
        let error =
            super::quality(&[], "[[query]]\ntext = \"x\"\nexpect = \"a b c\"\n").unwrap_err();
        assert!(error.to_string().contains("which is not a command"));
    }
}
