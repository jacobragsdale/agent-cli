use std::collections::BTreeMap;

use agent_cli_core::{Ctx, command, now, utc_time};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::config::Redis;
use crate::session::{cmd, on};

#[derive(clap::Args)]
pub struct ClientListArgs {
    /// Instance name from instance list (or its host, URL or connection string); defaults to the only one
    #[arg(long)]
    instance: Option<String>,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ClientRow {
    /// ip:port; on AKS, a pod's IP.
    addr: String,
    /// What the client named its connection (CLIENT SETNAME).
    name: Option<String>,
    user: Option<String>,
    db: Option<i64>,
    connected_at: Option<String>,
    last_command_at: Option<String>,
    /// The last command it ran.
    command: Option<String>,
    /// N normal, S a replica, M its primary, P pub/sub, b blocked, x in MULTI.
    flags: Option<String>,
}

// VERIFY(work): that Azure's access policies allow CLIENT LIST.
fn client_list(ctx: &Ctx, args: ClientListArgs) -> Result<Vec<ClientRow>> {
    let redis = Redis::load(ctx.config())?;
    let spec = redis.instance(args.instance.as_deref())?;
    let (own, text) = on(ctx, spec, |session| {
        let own = session.call(cmd!("CLIENT", "ID"))?.int();
        let text = session
            .call(cmd!("CLIENT", "LIST"))?
            .text()
            .unwrap_or_default();
        Ok((own, text))
    })?;
    let moment = now();
    let mut rows = Vec::new();
    for line in text.lines() {
        let fields: BTreeMap<&str, &str> = line
            .split_whitespace()
            .filter_map(|pair| pair.split_once('='))
            .collect();
        let field = |name: &str| {
            fields
                .get(name)
                .filter(|value| !value.is_empty())
                .map(|value| (*value).to_owned())
        };
        let int = |name: &str| fields.get(name).and_then(|value| value.parse::<i64>().ok());
        if int("id").is_some() && int("id") == own {
            continue;
        }
        let Some(addr) = field("addr") else {
            continue;
        };
        rows.push(ClientRow {
            addr,
            name: field("name"),
            user: field("user"),
            db: int("db"),
            connected_at: int("age").map(|age| utc_time(moment - time::Duration::seconds(age))),
            last_command_at: int("idle")
                .map(|idle| utc_time(moment - time::Duration::seconds(idle))),
            command: field("cmd").filter(|cmd| cmd != "NULL"),
            flags: field("flags"),
        });
    }
    if rows.len() > args.limit {
        let mut by_name: BTreeMap<&str, usize> = BTreeMap::new();
        for row in &rows {
            *by_name
                .entry(row.name.as_deref().unwrap_or("(no name)"))
                .or_default() += 1;
        }
        let mut counts: Vec<(&str, usize)> = by_name.into_iter().collect();
        counts.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
        let counts: Vec<String> = counts
            .iter()
            .take(10)
            .map(|(name, count)| format!("{name} {count}"))
            .collect();
        ctx.note(format!(
            "[{} of {} clients; by name: {}; --limit N]",
            args.limit,
            rows.len(),
            counts.join(", ")
        ));
        rows.truncate(args.limit);
    }
    Ok(rows)
}

command! {
    pub CLIENT_LIST = ["redis", "client", "list"], Read,
    "List who is connected to a Redis instance: address, name, idle, last command",
    keywords: ["connections", "connected", "apps", "pods", "idle", "who", "sessions"],
    example: "redis client list --instance orders-prod --fields addr,name,last_command_at",
    run: client_list,
}

#[cfg(test)]
mod tests {
    use crate::session::Reply;
    use crate::testing::{bulk, config, redis, serve};

    #[test]
    fn clients_print_without_our_own_connection_and_a_cut_counts_them_by_name() {
        let lines: String = (1..=4)
            .map(|id| {
                let name = if id == 4 { "agent-cli" } else if id % 2 == 0 { "worker" } else { "orders-api" };
                format!("id={id} addr=10.244.1.{id}:5000 laddr=10.0.0.4:6379 fd=8 name={name} age=600 idle=2 flags=N db=0 cmd=get user=default\n")
            })
            .collect();
        let fake = serve(vec![
            ("CLIENT ID", Reply::Int(4)),
            ("CLIENT LIST", bulk(lines)),
        ]);
        let outcome = redis(
            &["redis", "client", "list", "--limit", "2"],
            &config(&[("prod", fake.port)]),
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        let rows = outcome.json();
        assert_eq!(rows.as_array().unwrap().len(), 2);
        assert_eq!(
            (
                rows[0]["addr"].as_str(),
                rows[0]["name"].as_str(),
                rows[0]["command"].as_str()
            ),
            (Some("10.244.1.1:5000"), Some("orders-api"), Some("get"))
        );
        assert!(
            outcome
                .stderr
                .contains("[2 of 3 clients; by name: orders-api 2, worker 1; --limit N]"),
            "{}",
            outcome.stderr
        );
    }
}
