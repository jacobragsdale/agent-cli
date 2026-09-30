//! The project's Git repositories.

pub(crate) mod get;
pub(crate) mod list;

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{BASE, CODE, ado, page, urls};

    #[test]
    fn repo_list_filters_sorts_and_limits_and_repo_get_lists_branches() {
        let (outcome, _) = ado(
            &["ado", "repo", "list", "WEB", "--limit", "1"],
            vec![page(vec![
                json!({"id": "r-2", "name": "web-e2e", "defaultBranch": "refs/heads/main", "isDisabled": false}),
                json!({"id": "r-1", "name": "web", "defaultBranch": "refs/heads/main", "size": 2048,
                    "webUrl": format!("{BASE}/Fabrikam/_git/web")}),
                json!({"id": "r-3", "name": "api"}),
            ])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"name": "web", "id": "r-1", "default_branch": "main", "size": 2048,
                "web_url": format!("{BASE}/Fabrikam/_git/web")}])
        );
        assert_eq!(
            outcome.stderr,
            "[1 of 2; --limit N, or narrow with PATTERN]\n"
        );

        let (outcome, transport) = ado(
            &["ado", "repo", "get", "web"],
            vec![
                Answer::json(
                    &json!({"id": "r-1", "name": "web", "project": {"name": "Fabrikam"},
                    "defaultBranch": "refs/heads/main", "remoteUrl": format!("{BASE}/Fabrikam/_git/web")}),
                ),
                page(vec![
                    json!({"name": "refs/heads/main"}),
                    json!({"name": "refs/heads/feature/x"}),
                ]),
            ],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let repo = outcome.json();
        assert_eq!(repo["branches"], json!(["main", "feature/x"]));
        assert_eq!(repo["default_branch"], "main");
        assert_eq!(
            urls(&transport)[1],
            format!("{CODE}/git/repositories/r-1/refs?filter=heads/&$top=101&api-version=7.1")
        );
    }
}
