//! `net ip list`: public IPs with what holds each and, for one AKS made,
//! the k8s service it serves, so "whose IP is this" is one call.

use agent_cli_core::{Ctx, command};
use anyhow::Result;

use crate::config::Azure;
use crate::net::Place;
use crate::net::ip::{PublicIp, public_ips};

#[derive(clap::Args)]
pub struct IpListArgs {
    #[command(flatten)]
    place: Place,
    /// Most rows to return
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

fn ip_list(ctx: &Ctx, args: IpListArgs) -> Result<Vec<PublicIp>> {
    let azure = Azure::load(ctx)?;
    public_ips(ctx, &azure, &args.place, None, args.limit)
}

command! {
    pub IP_LIST = ["net", "ip", "list"], Read,
    "List public IPs with what holds each and the k8s service it serves",
    keywords: ["public", "ips", "owns", "owner", "egress", "outbound", "static"],
    example: "net ip list --fields address,held_by,service",
    run: ip_list,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::testing::{RG, azure, inventory};

    #[test]
    fn an_ip_aks_made_names_its_service_by_the_k8s_id() {
        let lb = format!("{RG}/providers/Microsoft.Network/loadBalancers/kubernetes");
        let (outcome, transport) = azure(
            &[crate::NET],
            &["net", "ip", "list"],
            vec![inventory(vec![
                json!({"id": "/ip/kubernetes-a1", "name": "kubernetes-a1", "resourceGroup": "mc_rg",
                    "address": "203.0.113.10", "sku": "Standard", "allocation": "Static",
                    "heldBy": format!("{lb}/frontendIPConfigurations/a1"),
                    "service": "web/nginx", "cluster": "aks-contoso-dev"}),
                json!({"id": "/ip/spare", "name": "spare", "resourceGroup": "rg-contoso",
                    "address": "", "sku": "Standard", "allocation": "Static", "heldBy": "",
                    "service": "", "cluster": ""}),
            ])],
        );
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(
            outcome.json(),
            json!([{"id": "/ip/kubernetes-a1", "name": "kubernetes-a1", "group": "mc_rg",
                "address": "203.0.113.10", "sku": "Standard", "allocation": "Static",
                "held_by": format!("{lb}/frontendIPConfigurations/a1"),
                "service": "aks-contoso-dev/web/nginx"},
                {"id": "/ip/spare", "name": "spare", "group": "rg-contoso", "sku": "Standard",
                "allocation": "Static"}])
        );
        let query = transport.sent()[0].body.clone().unwrap()["query"].clone();
        assert!(
            query
                .as_str()
                .unwrap()
                .contains("service = tostring(tags['k8s-azure-service'])"),
            "{query}"
        );
    }
}
