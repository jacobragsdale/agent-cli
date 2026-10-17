//! What the airflow set diffs share: two instances, each read into a map
//! by name.

use std::collections::BTreeMap;

use agent_cli_core::{Ctx, Diff, DiffArgs, Rules, Secret, Side, diff};
use anyhow::Result;
use serde_json::{Map, Value};

use crate::client::{Airflow, Client};

/// Two instances, by name.
#[derive(clap::Args)]
pub struct Instances {
    /// One [[airflow.instance]] by name (qa)
    left: String,
    /// The other, by name (prod)
    right: String,
    #[command(flatten)]
    pub(crate) diff: DiffArgs,
}

/// One instance's side: its rows by name, in the order printed, and the
/// values compared only for equality (`None` where Airflow masks them), by
/// path.
pub(crate) type Read = (Map<String, Value>, BTreeMap<String, Option<Secret>>);

impl Instances {
    /// Both instances read by `read`, compared under `rules`.
    pub(crate) fn compare(
        &self,
        ctx: &Ctx,
        rules: &Rules,
        read: impl Fn(&Client) -> Result<Read>,
    ) -> Result<Diff> {
        let airflow = Airflow::load(ctx.config())?;
        let side = |name: &str| -> Result<Side> {
            let client = airflow.open(ctx, Some(name))?;
            let (rows, secrets) = read(&client)?;
            let mut side = Side::new(client.instance.name.clone(), Value::Object(rows));
            side.secrets = secrets;
            Ok(side)
        };
        let (left, right) = (side(&self.left)?, side(&self.right)?);
        diff(ctx, &self.diff, rules, left, right)
    }
}

/// A value Airflow returns as `***` is masked: its equality is unknown.
pub(crate) fn unmasked(value: &Value) -> Option<Secret> {
    let text = match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    (text != "***").then(|| Secret::new(text))
}
