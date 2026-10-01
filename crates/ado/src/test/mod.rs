//! Test results: what a run's tests said, down to the line that failed. A
//! test task's timeline issue is only "3 tests failed"; the names, messages
//! and stacks are in the Test Results API.

pub(crate) mod list;

use agent_cli_core::Ctx;
use anyhow::Result;
use serde_json::Value;

use crate::client::{Ado, list, query_value};

/// A run's test runs (one per test task, usually): the Test Results API
/// names a build `vstfs:///Build/Build/ID`.
pub(crate) fn test_runs(ctx: &Ctx, ado: &Ado, build: i64) -> Result<Vec<Value>> {
    let uri = query_value(&format!("vstfs:///Build/Build/{build}"));
    let answer = ado.get(ctx, &ado.code("test/runs", &format!("buildUri={uri}")))?;
    Ok(list(&answer["value"]).to_vec())
}

/// Whether a test run counted tests that neither passed nor did not apply.
pub(crate) fn has_failures(run: &Value) -> bool {
    let count = |name: &str| run[name].as_i64().unwrap_or_default();
    count("totalTests") > count("passedTests") + count("notApplicableTests")
}
