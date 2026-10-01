use agent_cli_core::{Ctx, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Ado, short_branch, text};
use crate::ids::file_id;

use super::{definition, yaml_home};

#[derive(clap::Args)]
pub struct PipelineGetArgs {
    /// The pipeline's id or name
    pipeline: String,
}

/// A pipeline's definition: what it builds and from which YAML.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Pipeline {
    id: i64,
    name: Option<String>,
    folder: Option<String>,
    repo: Option<String>,
    default_branch: Option<String>,
    /// The file get id of its YAML file.
    yaml: Option<String>,
    /// enabled, paused or disabled.
    queue_status: Option<String>,
    url: Option<String>,
}

fn pipeline_get(ctx: &Ctx, args: PipelineGetArgs) -> Result<Pipeline> {
    let ado = Ado::load(ctx)?;
    let id = ado.pipeline_id(ctx, &args.pipeline)?;
    let found = definition(ctx, &ado, id)?;
    let repository = &found["repository"];
    Ok(Pipeline {
        id,
        name: text(&found["name"]),
        folder: text(&found["path"]),
        repo: text(&repository["name"]),
        default_branch: repository["defaultBranch"].as_str().map(short_branch),
        yaml: yaml_home(&found).map(|(repo, path)| file_id(None, &repo, None, &path, None)),
        queue_status: text(&found["queueStatus"]),
        url: text(&found["_links"]["web"]["href"]),
    })
}

command! {
    pub PIPELINE_GET = ["ado", "pipeline", "get"], Read,
    "Show a pipeline's definition: the YAML file it runs and its default branch",
    keywords: ["definition", "yml", "file", "configuration", "source"],
    example: "ado pipeline get api-ci --fields repo,yaml",
    run: pipeline_get,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{CODE, ado, urls};

    #[test]
    fn a_yaml_pipeline_names_its_file_as_file_get_takes_it() {
        let (outcome, transport) = ado(
            &["ado", "pipeline", "get", "12"],
            vec![Answer::json(
                &json!({"id": 12, "name": "web-ci", "path": "\\", "queueStatus": "enabled",
                "process": {"type": 2, "yamlFilename": "/ci/web.yml"},
                "repository": {"type": "TfsGit", "name": "web", "defaultBranch": "refs/heads/main"},
                "_links": {"web": {"href": "https://dev.azure.com/contoso/Fabrikam/_build/definition?definitionId=12"}}}),
            )],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!({"id": 12, "name": "web-ci", "folder": "\\", "repo": "web", "default_branch": "main",
                "yaml": "web:ci/web.yml", "queue_status": "enabled",
                "url": "https://dev.azure.com/contoso/Fabrikam/_build/definition?definitionId=12"})
        );
        assert_eq!(
            urls(&transport),
            [format!("{CODE}/build/definitions/12?api-version=7.1")]
        );

        let (outcome, _) = ado(
            &["ado", "pipeline", "get", "12"],
            vec![Answer::json(
                &json!({"id": 12, "process": {"yamlFilename": "a.yml"}, "repository": {"type": "GitHub", "name": "contoso/web"}}),
            )],
        );
        assert_eq!(
            outcome.json()["yaml"],
            json!(null),
            "a GitHub repository has no file get id"
        );
    }
}
