//! The `[redis]` section: one `[[redis.instance]]` per Redis an agent reads,
//! usually one per project and environment.
//!
//! ```toml
//! [[redis.instance]]
//! name = "orders-qa"       # what --instance takes; no colon
//! host = "contoso-orders-qa.redis.cache.windows.net"
//! auth = "az"              # Azure: your az login's Entra ID token
//!
//! [[redis.instance]]
//! name = "sessions-qa"     # self-hosted
//! host = "redis-qa.contoso.internal"
//! port = 6379              # default 6379; Azure Cache 6380, Azure Managed Redis 10000
//! tls = true               # default false; true for Azure's hosts
//! trust_cert = true        # accept a self-signed certificate
//! user = "agent"           # an ACL user; left out: default
//! password_cmd = "pass show contoso/redis-qa"
//! db = 0
//! ```
//!
//! Everything but the credential is checked when a redis command reads the
//! section; the credential is resolved only when a connection opens.

use agent_cli_core::{Config, Credential, Ctx, Failure, Secret, host_under, pick};
use anyhow::Result;
use serde::Deserialize;

/// The audience of an Entra ID token for Azure Cache for Redis and Azure
/// Managed Redis.
pub const AZURE_REDIS: &str = "https://redis.azure.com/";

/// The hosts an az token may go to, and the port each listens on for TLS.
const AZURE_HOSTS: &[(&str, u16)] = &[
    (".redis.cache.windows.net", 6380),
    (".redis.azure.net", 10000),
];

/// One `[[redis.instance]]`, checked, with the defaults filled in.
#[derive(Clone, Debug)]
pub struct Instance {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub trust_cert: bool,
    pub db: u32,
    /// `auth = "az"`: sign in with the az login's Entra ID token.
    pub az: bool,
    /// An ACL user, or with `az` the Entra object id (else the token's).
    pub user: Option<String>,
    pub password: Option<Credential>,
}

#[derive(Debug, Default)]
pub struct Redis {
    pub instances: Vec<Instance>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Section {
    #[serde(default)]
    instance: Vec<Raw>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    name: String,
    host: String,
    port: Option<u16>,
    tls: Option<bool>,
    trust_cert: Option<bool>,
    db: Option<u32>,
    auth: Option<String>,
    user: Option<String>,
    password: Option<String>,
    password_env: Option<String>,
    password_cmd: Option<String>,
}

impl Redis {
    /// The `[redis]` section of `config`; exit 3 naming the file when it is
    /// wrong.
    pub fn load(config: &Config) -> Result<Self> {
        let section: Section = config.section("redis")?;
        let fix = |message: String| -> anyhow::Error {
            Failure::setup(format!("[redis] in {}: {message}", config.path().display()))
                .hint("fix it; `agent-cli config example redis` shows every key")
                .into()
        };
        let mut instances: Vec<Instance> = Vec::with_capacity(section.instance.len());
        for raw in section.instance {
            let name = raw.name.clone();
            if instances.iter().any(|earlier| earlier.name == name) {
                return Err(fix(format!("instance {name:?} is named twice")));
            }
            instances.push(
                raw.check()
                    .map_err(|why| fix(format!("instance {name:?}: {why}")))?,
            );
        }
        Ok(Self { instances })
    }

    /// The instance `--instance` names: its name, or its host as an app
    /// is configured with it (`host`, `host:port`, a `redis://` or
    /// `rediss://` URL, a StackExchange.Redis connection string). The only
    /// one when none is named. A host no instance has is exit 2: agent-cli
    /// connects only where the config says.
    pub fn instance(&self, wanted: Option<&str>) -> Result<&Instance> {
        if self.instances.is_empty() {
            return Err(Failure::setup("no [[redis.instance]] is configured")
                .hint("agent-cli config example redis  (prints one to paste)")
                .into());
        }
        let by_name = |wanted: Option<&str>| {
            pick("instance", "--instance", wanted, &self.instances, |spec| {
                &spec.name
            })
            .map_err(|failure| failure.hint("agent-cli redis instance list").into())
        };
        let Some((host, port)) = wanted
            .filter(|wanted| !self.instances.iter().any(|spec| spec.name == *wanted))
            .and_then(endpoint)
        else {
            return by_name(wanted);
        };
        let at: Vec<&Instance> = self
            .instances
            .iter()
            .filter(|spec| {
                spec.host.eq_ignore_ascii_case(&host) && port.is_none_or(|port| port == spec.port)
            })
            .collect();
        match at.as_slice() {
            [one] => Ok(one),
            [] => Err(Failure::usage(format!(
                "no configured instance is at {host}{}; agent-cli connects only to a [[redis.instance]]",
                port.map(|port| format!(":{port}")).unwrap_or_default()
            ))
            .hint("agent-cli redis instance list")
            .into()),
            many => Err(Failure::usage(format!(
                "{host} is more than one instance; name one with --instance: {}",
                many.iter()
                    .map(|spec| spec.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
            .into()),
        }
    }
}

/// The host and port of a reference that is not a bare word: `rediss://u:p@h:6380/0`,
/// `h:6380,password=…,ssl=True`, `h:6380`, `h.contoso.example`.
fn endpoint(wanted: &str) -> Option<(String, Option<u16>)> {
    let (rest, url) = match wanted
        .strip_prefix("rediss://")
        .or_else(|| wanted.strip_prefix("redis://"))
    {
        Some(rest) => (rest, true),
        None => (wanted, false),
    };
    let rest = rest.rsplit_once('@').map_or(rest, |(_, host)| host);
    let rest = rest.split([',', '/', '?']).next()?;
    let (host, port) = match rest.rsplit_once(':') {
        Some((host, port)) => (host, Some(port.parse().ok()?)),
        None => (rest, None),
    };
    let looks = url || port.is_some() || wanted.contains(',') || host.contains('.');
    (looks && !host.is_empty()).then(|| (host.to_owned(), port))
}

impl Raw {
    fn check(self) -> std::result::Result<Instance, String> {
        if self.name.trim().is_empty() {
            return Err("name is empty; --instance picks an instance by it".to_owned());
        }
        if self.name.contains(':') {
            return Err(
                "a name holds no colon: a diff side is INSTANCE:KEY, split at the first one"
                    .to_owned(),
            );
        }
        let host = self.host.trim().to_owned();
        if host.is_empty() {
            return Err("host is empty".to_owned());
        }
        let azure = AZURE_HOSTS
            .iter()
            .find(|(suffix, _)| host_under(&format!("https://{host}/"), suffix));
        let tls = self.tls.unwrap_or(azure.is_some());
        let port = match self.port {
            Some(0) => return Err("port 0 is not a port".to_owned()),
            Some(port) => port,
            None => azure.filter(|_| tls).map_or(6379, |(_, port)| *port),
        };
        if self.trust_cert.is_some() && !tls {
            return Err("trust_cert is for tls = true".to_owned());
        }
        let password = Credential::from_keys(
            "password",
            self.password,
            self.password_env,
            self.password_cmd,
        )?;
        let az = match self.auth.as_deref() {
            None | Some("password") => false,
            Some("az") => true,
            Some(other) => return Err(format!("auth {other:?} is not \"password\" or \"az\"")),
        };
        if az {
            if azure.is_none() {
                return Err(format!(
                    "auth \"az\" sends your Entra ID token, so only to a host under {}; {host} is not",
                    AZURE_HOSTS
                        .iter()
                        .map(|(suffix, _)| &suffix[1..])
                        .collect::<Vec<_>>()
                        .join(" or ")
                ));
            }
            if !tls {
                return Err("auth \"az\" needs tls".to_owned());
            }
            if password.is_some() {
                return Err(
                    "auth \"az\" signs in with your az login: leave out the password".to_owned(),
                );
            }
        }
        Ok(Instance {
            name: self.name,
            host,
            port,
            tls,
            trust_cert: self.trust_cert.unwrap_or(false),
            db: self.db.unwrap_or(0),
            az,
            user: self.user.filter(|user| !user.trim().is_empty()),
            password,
        })
    }
}

impl Instance {
    /// Where it signs in from, for doctor: never the value.
    pub fn source(&self) -> String {
        match (&self.password, self.az) {
            (_, true) => "your az login's Entra ID token".to_owned(),
            (Some(credential), _) => credential.source(),
            (None, _) => "no password".to_owned(),
        }
    }

    /// The user and password `AUTH` sends, resolved now that the
    /// connection is open; `None` for an instance with no password. With
    /// `az`, the token for [`AZURE_REDIS`] (minted again when `fresh`), and
    /// the object id it was issued to unless `user` names one.
    pub fn credentials(&self, ctx: &Ctx, fresh: bool) -> Result<Option<(Option<String>, Secret)>> {
        let named = |error: anyhow::Error| error.context(format!("instance {:?}", self.name));
        if self.az {
            let token = ctx.az_token(AZURE_REDIS, fresh).map_err(named)?;
            let user = match &self.user {
                Some(user) => user.clone(),
                None => object_id(token.expose()).ok_or_else(|| {
                    named(
                        Failure::setup("the az token names no object id (oid) to sign in as")
                            .hint("set user = \"OBJECT-ID\" in its [[redis.instance]]")
                            .into(),
                    )
                })?,
            };
            return Ok(Some((Some(user), token)));
        }
        match &self.password {
            None => Ok(None),
            Some(credential) => Ok(Some((
                self.user.clone(),
                credential.resolve(ctx).map_err(named)?,
            ))),
        }
    }
}

/// The `oid` claim of a JWT: Azure's Redis signs in the token's principal
/// by its object id. Read, not verified: the token came from az, and Redis
/// verifies it.
// VERIFY(work): on Azure Cache and on Azure Managed Redis.
fn object_id(token: &str) -> Option<String> {
    let payload = base64url(token.split('.').nth(1)?)?;
    let claims: serde_json::Value = serde_json::from_slice(&payload).ok()?;
    claims["oid"].as_str().map(str::to_owned)
}

fn base64url(text: &str) -> Option<Vec<u8>> {
    let (mut out, mut acc, mut bits) = (Vec::new(), 0u32, 0);
    for byte in text.bytes().filter(|byte| *byte != b'=') {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use agent_cli_core::Exit;

    use super::*;

    fn load(toml: &str) -> Result<Redis> {
        Redis::load(&Config::parse("/cfg/config.toml", Some(toml), Vec::new()))
    }

    fn instance(fields: &str) -> std::result::Result<Instance, String> {
        load(&format!(
            "[[redis.instance]]\nname = \"orders-qa\"\n{fields}\n"
        ))
        .map(|redis| redis.instances[0].clone())
        .map_err(|error| format!("{error:#}"))
    }

    #[test]
    fn azure_hosts_default_to_tls_on_their_own_ports_and_others_to_plain_6379() {
        let cache =
            instance("host = \"contoso-orders.redis.cache.windows.net\"\nauth = \"az\"").unwrap();
        assert_eq!((cache.port, cache.tls, cache.az), (6380, true, true));
        let managed =
            instance("host = \"contoso-orders.eastus.redis.azure.net\"\nauth = \"az\"").unwrap();
        assert_eq!((managed.port, managed.tls), (10000, true));
        let plain = instance("host = \"redis-qa.contoso.internal\"").unwrap();
        assert_eq!((plain.port, plain.tls, plain.db), (6379, false, 0));
        let tls = instance(
            "host = \"redis-qa.contoso.internal\"\ntls = true\nport = 6380\ntrust_cert = true",
        )
        .unwrap();
        assert_eq!((tls.port, tls.tls, tls.trust_cert), (6380, true, true));
    }

    #[test]
    fn every_mistake_names_the_instance_and_is_exit_three() {
        for (fields, want) in [
            (
                "host = \"h.contoso.internal\"\nauth = \"az\"",
                "only to a host under redis.cache.windows.net or redis.azure.net",
            ),
            (
                "host = \"evil.redis.cache.windows.net.contoso.example\"\nauth = \"az\"",
                "only to a host under",
            ),
            (
                "host = \"c.redis.cache.windows.net\"\nauth = \"az\"\ntls = false",
                "auth \"az\" needs tls",
            ),
            (
                "host = \"c.redis.cache.windows.net\"\nauth = \"az\"\npassword_env = \"P\"",
                "leave out the password",
            ),
            (
                "host = \"h\"\ntrust_cert = true",
                "trust_cert is for tls = true",
            ),
            ("host = \"h\"\nport = 0", "port 0 is not a port"),
            (
                "host = \"h\"\nauth = \"kerberos\"",
                "not \"password\" or \"az\"",
            ),
            (
                "host = \"h\"\npassword = \"a\"\npassword_cmd = \"b\"",
                "give one of password",
            ),
            ("host = \"\"", "host is empty"),
            ("host = \"h\"\npasword = \"x\"", "unknown field `pasword`"),
        ] {
            let message = instance(fields).unwrap_err();
            assert!(message.contains(want), "{fields}: {message}");
        }
        let error = load("[[redis.instance]]\nname = \"qa:1\"\nhost = \"h\"\n").unwrap_err();
        assert!(format!("{error:#}").contains("holds no colon"));
        let exit = error
            .chain()
            .find_map(|cause| cause.downcast_ref::<Failure>())
            .map(|failure| failure.exit);
        assert_eq!(exit, Some(Exit::Setup));
        let twice = "[[redis.instance]]\nname = \"a\"\nhost = \"h\"\n";
        assert!(format!("{:#}", load(&twice.repeat(2)).unwrap_err()).contains("named twice"));
    }

    #[test]
    fn an_instance_is_found_by_name_host_url_or_connection_string() {
        let redis = load(
            "[[redis.instance]]\nname = \"orders-qa\"\nhost = \"contoso-orders-qa.redis.cache.windows.net\"\nauth = \"az\"\n\
             [[redis.instance]]\nname = \"orders-prod\"\nhost = \"contoso-orders.eastus.redis.azure.net\"\nauth = \"az\"\n\
             [[redis.instance]]\nname = \"local\"\nhost = \"127.0.0.1\"\n",
        )
        .unwrap();
        let name = |wanted: &str| redis.instance(Some(wanted)).map(|spec| spec.name.clone());
        for (wanted, want) in [
            ("orders-qa", "orders-qa"),
            ("contoso-orders-qa.redis.cache.windows.net", "orders-qa"),
            (
                "CONTOSO-ORDERS-QA.redis.cache.windows.net:6380",
                "orders-qa",
            ),
            (
                "contoso-orders-qa.redis.cache.windows.net:6380,password=hunter2,ssl=True,abortConnect=False",
                "orders-qa",
            ),
            (
                "rediss://:hunter2@contoso-orders.eastus.redis.azure.net:10000/0",
                "orders-prod",
            ),
            ("redis://127.0.0.1", "local"),
            ("127.0.0.1:6379", "local"),
        ] {
            assert_eq!(name(wanted).unwrap(), want, "{wanted}");
        }
        for (wanted, want) in [
            (
                "contoso-orders-qa.redis.cache.windows.net:6379",
                "no configured instance is at contoso-orders-qa.redis.cache.windows.net:6379",
            ),
            (
                "other.contoso.example,password=hunter2",
                "no configured instance is at other.contoso.example;",
            ),
            (
                "nope",
                "--instance takes one of: orders-qa, orders-prod, local",
            ),
        ] {
            let message = format!("{:#}", redis.instance(Some(wanted)).unwrap_err());
            assert!(
                message.contains(want) && !message.contains("hunter2"),
                "{wanted}: {message}"
            );
        }
        assert!(
            format!("{:#}", redis.instance(None).unwrap_err()).contains("more than one instance")
        );
        assert!(
            format!("{:#}", load("").unwrap().instance(None).unwrap_err())
                .contains("no [[redis.instance]]")
        );
    }

    #[test]
    fn auth_az_signs_in_as_the_tokens_object_id_unless_a_user_is_named() {
        assert_eq!(
            base64url("eyJvaWQiOiJhYmMifQ").unwrap(),
            br#"{"oid":"abc"}"#
        );
        assert_eq!(
            object_id("x.eyJvaWQiOiIwMDAwMDAwMC0wMDAwLTQwMDAtODAwMC0wMDAwMDAwMDAwMDEifQ.y")
                .as_deref(),
            Some("00000000-0000-4000-8000-000000000001")
        );
        assert_eq!(object_id("not-a-jwt"), None);
        let ctx = agent_cli_core::testing::ctx(agent_cli_core::Setup::fake(
            agent_cli_core::testing::FakeTransport::default(),
        ));
        let mut spec = instance("host = \"c.redis.cache.windows.net\"\nauth = \"az\"").unwrap();
        let message = format!("{:#}", spec.credentials(&ctx, false).unwrap_err());
        assert!(message.contains("names no object id"), "{message}");
        spec.user = Some("00000000-0000-4000-8000-000000000001".to_owned());
        let (user, token) = spec.credentials(&ctx, true).unwrap().unwrap();
        assert_eq!(
            user.as_deref(),
            Some("00000000-0000-4000-8000-000000000001")
        );
        assert_eq!(token.expose(), "token-fresh@https://redis.azure.com/");
        let open = instance("host = \"h\"").unwrap();
        assert!(open.credentials(&ctx, false).unwrap().is_none());
        assert_eq!(open.source(), "no password");
    }
}
