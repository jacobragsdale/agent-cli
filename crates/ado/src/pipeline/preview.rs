use std::path::PathBuf;

use agent_cli_core::{Ctx, Failure, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;

use crate::client::{Ado, PREVIEW_API, full_ref, text};
use crate::ids::file_id;

use super::{definition, yaml_home};

#[derive(clap::Args)]
pub struct PipelinePreviewArgs {
    /// The pipeline's id or name
    pipeline: String,
    /// YAML to expand instead of the pipeline's own file, or - to read stdin
    #[arg(allow_hyphen_values = true)]
    yaml: Option<String>,
    /// The YAML to expand, from a file
    #[arg(long)]
    yaml_file: Option<PathBuf>,
    /// The branch its file and templates come from (default: the pipeline's default branch)
    #[arg(long)]
    branch: Option<String>,
}

/// The YAML a run would execute.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Preview {
    pipeline: i64,
    branch: Option<String>,
    /// Templates inlined and expressions resolved.
    yaml: String,
}

/// The most expanded lines shown.
const MAX_LINES: usize = 2000;

fn pipeline_preview(ctx: &Ctx, args: PipelinePreviewArgs) -> Result<Preview> {
    let yaml = ctx.long_text(
        "yaml",
        args.yaml.as_deref(),
        args.yaml_file.as_deref(),
        None,
    )?;
    let ado = Ado::load(ctx)?;
    let id = ado.pipeline_id(ctx, &args.pipeline)?;
    let mut body = json!({"previewRun": true});
    if let Some(yaml) = &yaml {
        body["yamlOverride"] = json!(yaml.text);
    }
    if let Some(branch) = &args.branch {
        body["resources"] = json!({"repositories": {"self": {"refName": full_ref(branch)}}});
    }
    let url = ado.api(
        Some(&ado.code_project),
        &format!("pipelines/{id}/preview"),
        "",
        PREVIEW_API,
    );
    let answer = ado.query(ctx, &url, body).map_err(|error| {
        hint_the_line(ctx, &ado, id, args.branch.as_deref(), yaml.is_some(), error)
    })?;
    let expanded = text(&answer["finalYaml"]).unwrap_or_default();
    let lines: Vec<&str> = expanded.lines().collect();
    if lines.len() > MAX_LINES {
        ctx.note(format!("[first {MAX_LINES} of {} lines]", lines.len()));
    }
    Ok(Preview {
        pipeline: id,
        branch: args.branch,
        yaml: lines[..lines.len().min(MAX_LINES)].join("\n"),
    })
}

/// A YAML error names `PATH (Line: N, Col: M)`. In a file of the pipeline's
/// repository (a template, or its own file when no --yaml replaced it), the
/// hint is that line's `file get`.
fn hint_the_line(
    ctx: &Ctx,
    ado: &Ado,
    id: i64,
    branch: Option<&str>,
    replaced: bool,
    error: anyhow::Error,
) -> anyhow::Error {
    let Some((path, line)) = named_line(&format!("{error:#}")) else {
        return error;
    };
    let failure = match error.downcast::<Failure>() {
        Ok(failure) => failure,
        Err(error) => return error,
    };
    let Some((repo, root)) = definition(ctx, ado, id).ok().as_ref().and_then(yaml_home) else {
        return failure.into();
    };
    if replaced && path == root {
        return failure
            .hint(format!("the line is {line} of the YAML you passed"))
            .into();
    }
    failure
        .hint(format!(
            "agent-cli ado file get {}",
            file_id(None, &repo, branch, &path, Some((line, line)))
        ))
        .into()
}

/// `/templates/test.yml (Line: 14, Col: 5): …` → (`templates/test.yml`, 14);
/// a template from another repository resource (`x.yml@tools`) names none.
fn named_line(message: &str) -> Option<(String, usize)> {
    let (before, after) = message.split_once(" (Line: ")?;
    let path = before.rsplit(' ').next()?.trim_start_matches('/');
    let path = path.strip_suffix("@self").unwrap_or(path);
    let line = after
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()?;
    (!path.contains('@') && !path.is_empty()).then(|| (path.to_owned(), line))
}

command! {
    pub PIPELINE_PREVIEW = ["ado", "pipeline", "preview"], Read,
    "Expand a pipeline's YAML, or an edit of it, without queuing anything",
    keywords: ["validate", "lint", "check", "template", "expand", "dry", "syntax", "yml"],
    example: "ado pipeline preview api-ci --yaml-file azure-pipelines.yml",
    run: pipeline_preview,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{CODE, ado, ado_piped};

    fn definition() -> Answer {
        Answer::json(
            &json!({"id": 12, "process": {"yamlFilename": "azure-pipelines.yml"},
            "repository": {"type": "TfsGit", "name": "web"}}),
        )
    }

    #[test]
    fn a_preview_expands_the_yaml_given_on_a_branch_without_running_it() {
        let (outcome, transport) = ado_piped(
            "steps:\n- template: t.yml\n",
            &["ado", "pipeline", "preview", "12", "-", "--branch", "dev"],
            vec![Answer::json(
                &json!({"finalYaml": "steps:\n- bash: make\n"}),
            )],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"pipeline": 12, "branch": "dev", "yaml": "steps:\n- bash: make"})
        );
        let sent = transport.sent();
        assert!(sent[0].method.is_read(), "a preview runs nothing");
        assert_eq!(
            sent[0].url,
            format!("{CODE}/pipelines/12/preview?api-version=7.1-preview.1")
        );
        assert_eq!(
            sent[0].body.clone().unwrap(),
            json!({"previewRun": true, "yamlOverride": "steps:\n- template: t.yml",
                "resources": {"repositories": {"self": {"refName": "refs/heads/dev"}}}})
        );
    }

    #[test]
    fn a_template_error_is_exit_2_and_its_hint_reads_that_line() {
        let refused = |message: &str| {
            Answer::status(
                400,
                json!({"message": message, "typeKey": "PipelineValidationException"}).to_string(),
            )
        };
        let (outcome, _) = ado(
            &["ado", "pipeline", "preview", "12", "--branch", "dev"],
            vec![
                refused("/templates/test.yml (Line: 14, Col: 5): Unexpected value 'step'"),
                definition(),
            ],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome.stderr.contains("Unexpected value 'step'"),
            "{}",
            outcome.stderr
        );
        assert!(
            outcome
                .stderr
                .contains("hint: agent-cli ado file get web@dev:templates/test.yml:14"),
            "{}",
            outcome.stderr
        );

        let (outcome, _) = ado(
            &["ado", "pipeline", "preview", "12", "pool: x"],
            vec![
                refused("/azure-pipelines.yml (Line: 1, Col: 7): Unexpected value 'x'"),
                definition(),
            ],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert!(
            outcome
                .stderr
                .contains("the line is 1 of the YAML you passed"),
            "{}",
            outcome.stderr
        );

        let (outcome, transport) = ado(
            &["ado", "pipeline", "preview", "12"],
            vec![refused("/x.yml@tools (Line: 2, Col: 1): bad")],
        );
        assert_eq!(outcome.code, 2, "{outcome:?}");
        assert_eq!(
            transport.sent().len(),
            1,
            "another repository's template is not looked up"
        );
    }
}
