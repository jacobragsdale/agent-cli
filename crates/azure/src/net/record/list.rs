//! `net record list`: a DNS zone's record sets, public or private, each with
//! its values in one list whatever its type, since public and private DNS
//! spell the same record differently (`ARecords`, `aRecords`).

use agent_cli_core::{Ctx, Request, command};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::client::{ARM, arm_only, bearer, limited, text};
use crate::config::Azure;
use crate::net::{DNS_API, PRIVATE_DNS_API, PRIVATE_ZONE, Place, ZONE, list, read};

#[derive(clap::Args)]
pub struct RecordListArgs {
    /// The DNS zone, public or private: its ARM id, portal link, or name
    zone: String,
    /// Part of the record's name: kv-contoso, api
    name: Option<String>,
    #[command(flatten)]
    place: Place,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RecordRow {
    /// Relative to the zone; `@` is the zone itself.
    name: String,
    /// A, AAAA, CNAME, MX, NS, PTR, SOA, SRV, TXT or CAA.
    #[serde(rename = "type")]
    kind: String,
    ttl: Option<u64>,
    /// Addresses, names or texts; MX, SRV, SOA and CAA as their fields in zone-file order.
    values: Vec<String>,
}

/// A record's values, whatever case its keys are in.
fn values(props: &Value) -> Vec<String> {
    let field = |item: &Value, name: &str| -> String {
        let found = item.as_object().and_then(|fields| {
            fields
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, held)| held)
        });
        match found {
            Some(Value::String(held)) => held.clone(),
            Some(Value::Array(parts)) => parts.iter().filter_map(Value::as_str).collect(),
            Some(Value::Null) | None => String::new(),
            Some(other) => other.to_string(),
        }
    };
    let mut found = Vec::new();
    for (key, held) in props.as_object().into_iter().flatten() {
        let key = key.to_ascii_lowercase();
        let Some(kind) = key
            .strip_suffix("records")
            .or_else(|| key.strip_suffix("record"))
        else {
            continue;
        };
        let items = match held {
            Value::Array(items) => items.iter().collect(),
            Value::Object(_) => vec![held],
            _ => continue,
        };
        for item in items {
            let get = |name: &str| field(item, name);
            found.push(match kind {
                "a" => get("ipv4Address"),
                "aaaa" => get("ipv6Address"),
                "cname" => get("cname"),
                "ns" => get("nsdname"),
                "ptr" => get("ptrdname"),
                "txt" => get("value"),
                "mx" => format!("{} {}", get("preference"), get("exchange")),
                "srv" => format!(
                    "{} {} {} {}",
                    get("priority"),
                    get("weight"),
                    get("port"),
                    get("target")
                ),
                "soa" => format!("{} {} {}", get("host"), get("email"), get("serialNumber")),
                "caa" => format!("{} {} {}", get("flags"), get("tag"), get("value")),
                _ => item.to_string(),
            });
        }
    }
    found
}

fn record_list(ctx: &Ctx, args: RecordListArgs) -> Result<Vec<RecordRow>> {
    let azure = Azure::load(ctx)?;
    let zone = args.place.find(
        ctx,
        &azure,
        &args.zone,
        &[PRIVATE_ZONE, ZONE],
        "net zone list",
        "net record list",
    )?;
    let (path, api) = if zone.kind().eq_ignore_ascii_case(PRIVATE_ZONE) {
        (format!("{}/ALL", zone.id), PRIVATE_DNS_API)
    } else {
        (format!("{}/recordsets", zone.id), DNS_API)
    };
    let wanted = args.name.as_deref().map(str::to_ascii_lowercase);
    let mut rows = Vec::new();
    let mut page = read(ctx, &path, api, "net zone list")?;
    loop {
        rows.extend(
            list(&page["value"])
                .iter()
                .map(|set| RecordRow {
                    name: text(&set["name"]).unwrap_or_default(),
                    kind: text(&set["type"])
                        .and_then(|kind| kind.rsplit('/').next().map(str::to_owned))
                        .unwrap_or_default(),
                    ttl: set["properties"]["TTL"]
                        .as_u64()
                        .or(set["properties"]["ttl"].as_u64()),
                    values: values(&set["properties"]),
                })
                .filter(|row| {
                    wanted
                        .as_deref()
                        .is_none_or(|part| row.name.to_ascii_lowercase().contains(part))
                }),
        );
        let Some(next) = text(&page["nextLink"]).filter(|_| rows.len() <= args.limit) else {
            break;
        };
        page = ctx
            .read(Request::get(arm_only(&next)?).auth(&bearer(ctx, ARM)))?
            .json()?;
    }
    Ok(limited(ctx, rows, args.limit))
}

command! {
    pub RECORD_LIST = ["net", "record", "list"], Read,
    "List a DNS zone's records with their type, TTL and values, public or private",
    keywords: ["records", "a", "cname", "resolve", "resolves", "fqdn", "hostname", "address"],
    example: "net record list privatelink.vaultcore.azure.net --fields name,type,values",
    run: record_list,
}

#[cfg(test)]
mod tests {
    use agent_cli_core::testing::Answer;
    use serde_json::json;

    use crate::testing::{RG, azure};

    #[test]
    fn private_and_public_records_read_alike_and_a_name_filters() {
        let zone = format!(
            "{RG}/providers/Microsoft.Network/privateDnsZones/privatelink.vaultcore.azure.net"
        );
        let (outcome, transport) = azure(
            &[crate::NET],
            &["net", "record", "list", &zone, "KV"],
            vec![Answer::json(&json!({"value": [
                {"name": "@", "type": "Microsoft.Network/privateDnsZones/SOA", "properties": {"ttl": 3600,
                    "soaRecord": {"host": "azureprivatedns.net", "email": "azureprivatedns-host.microsoft.com", "serialNumber": 1}}},
                {"name": "kv-contoso", "type": "Microsoft.Network/privateDnsZones/A", "properties": {"ttl": 10,
                    "aRecords": [{"ipv4Address": "10.0.4.4"}]}},
            ]}))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"name": "kv-contoso", "type": "A", "ttl": 10, "values": ["10.0.4.4"]}])
        );
        assert_eq!(
            transport.sent()[0].url,
            format!("https://management.azure.com{zone}/ALL?api-version=2020-06-01")
        );

        let public = format!("{RG}/providers/Microsoft.Network/dnszones/contoso.example");
        let (outcome, transport) = azure(
            &[crate::NET],
            &["net", "record", "list", &public],
            vec![Answer::json(&json!({"value": [
                {"name": "@", "type": "Microsoft.Network/dnszones/MX", "properties": {"TTL": 3600,
                    "MXRecords": [{"preference": 10, "exchange": "mail.contoso.example"}]}},
                {"name": "www", "type": "Microsoft.Network/dnszones/TXT", "properties": {"TTL": 300,
                    "TXTRecords": [{"value": ["v=spf1 ", "-all"]}]}},
            ]}))],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"name": "@", "type": "MX", "ttl": 3600, "values": ["10 mail.contoso.example"]},
                {"name": "www", "type": "TXT", "ttl": 300, "values": ["v=spf1 -all"]}])
        );
        assert!(
            transport.sent()[0]
                .url
                .ends_with("/recordsets?api-version=2018-05-01")
        );
    }
}
