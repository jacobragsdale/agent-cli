//! `__PATH__`: TODO say what it does and which API call it makes.
//!
//! Written by scripts/new-command.sh. It compiles and its tests fail until
//! every TODO is filled in; docs/how-to/add-a-command.md walks each step.

use agent_cli_core::{Ctx, Request, command}; //@read
use agent_cli_core::{Ctx, Effect, Method, Request, command}; //@write
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

#[derive(clap::Args)]
pub struct __ARGS__ {
    /// Most rows to return //@list
    #[arg(long, default_value_t = 50)] //@list
    limit: usize, //@list
    /// TODO: the id `__DOMAIN__ __RESOURCE__ list` prints //@one
    id: String, //@one
}

/// TODO: the fields an agent wants, `id` first: exactly what `get` takes.
#[derive(Debug, Serialize, JsonSchema)]
pub struct __ROW__ {
    id: String,
}

fn __HANDLER__(ctx: &Ctx, args: __ARGS__) -> Result<Vec<__ROW__>> { //@list
fn __HANDLER__(ctx: &Ctx, args: __ARGS__) -> Result<__ROW__> { //@one
    // TODO: send through the domain's client instead (its card names the
    // helpers): it builds the URL, attaches the credential and checks the host.
    let url = "https://example.invalid/TODO".to_owned();
    let answer = ctx.read(Request::get(url))?.json()?; //@read
    // TODO: Effect::Write if it is easy to undo; Varies decides per input. //@write
    let request = Request::new(Method::Post, url).json(serde_json::json!({})); //@write
    let answer = ctx.write(Effect::Destructive, request)?.json()?; //@write
    let _ = (answer, args.limit); //@list
    Ok(Vec::new()) //@list
    let _ = answer; //@one
    Ok(__ROW__ { id: args.id }) //@one
}

command! {
    pub __CONST__ = ["__DOMAIN__", "__RESOURCE__", "__VERB__"], __EFFECT__,
    "TODO say what it does, imperative, at most 80 characters",
    keywords: [],
    example: "__DOMAIN__ __RESOURCE__ __VERB__ --fields id", //@list
    example: "__DOMAIN__ __RESOURCE__ __VERB__ TODO-ID__YES__", //@one
    run: __HANDLER__,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::Setup;
    use agent_cli_core::testing::{Answer, FakeTransport, assert_dry_run, run}; //@write
    use agent_cli_core::testing::{Answer, FakeTransport, run}; //@read
    use serde_json::json;

    const ARGV: &[&str] = &["__DOMAIN__", "__RESOURCE__", "__VERB__"]; //@list
    const ARGV: &[&str] = &["__DOMAIN__", "__RESOURCE__", "__VERB__", "TODO-ID"]; //@one

    #[test]
    fn __HANDLER___prints_what_the_service_answers() {
        let transport = FakeTransport::answering([Answer::json(&json!({"TODO": "a real answer"}))]);
        let argv = [ARGV, &["__YES_ARG__"]].concat(); //@yes
        let outcome = run(&[crate::__DOMAIN_CONST__], &argv, Setup::fake(transport.clone())); //@yes
        let outcome = run(&[crate::__DOMAIN_CONST__], ARGV, Setup::fake(transport.clone())); //@noyes
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.json(), json!("TODO: what it prints"));
        assert_eq!(transport.sent()[0].url, "TODO: the URL it asked");
    }

    #[test] //@write
    fn __HANDLER___plans_its_change_under_dry_run() { //@write
        let plans = assert_dry_run(&[crate::__DOMAIN_CONST__], ARGV, vec![]); //@write
        assert_eq!(plans[0]["url"], "TODO: the URL it would change"); //@write
    } //@write
}
