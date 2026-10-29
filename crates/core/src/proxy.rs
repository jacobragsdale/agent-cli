//! The proxy a request goes out through, chosen by core for every domain:
//! `[core] proxy`, else the `HTTPS_PROXY` family read through `ctx.env`,
//! less what `no_proxy` (or `NO_PROXY`) exempts.
//!
//! ureq would read those variables itself, but then doctor could not name
//! the proxy, tests could not set it, and a refusal would read as a broken
//! connection; so core decides per request and hands ureq the answer.

use std::io::{Read, Write};
use std::net::{IpAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

use anyhow::Result;
use toml::Value;

use crate::credential::Credential;
use crate::ctx::{Ctx, locked};
use crate::error::Failure;
use crate::secret::Secret;

/// The proxy core picked for one request.
#[derive(Clone, Debug)]
pub struct Via {
    /// `http://host:port`, never with a user in it: what rows, plans and
    /// doctor show.
    pub url: String,
    /// Where it was set: `[core] proxy`, `HTTPS_PROXY`.
    pub from: String,
    pub user: Option<String>,
    pub password: Option<Secret>,
}

impl Via {
    /// The URL ureq takes, the sign-in percent-encoded into it.
    pub(crate) fn wire(&self) -> String {
        let Some(user) = &self.user else {
            return self.url.clone();
        };
        let (scheme, rest) = self.url.split_once("://").unwrap_or(("http", &self.url));
        let mut userinfo = String::new();
        crate::http::percent_encode(user, &mut userinfo);
        if let Some(password) = &self.password {
            userinfo.push(':');
            crate::http::percent_encode(password.expose(), &mut userinfo);
        }
        format!("{scheme}://{userinfo}@{rest}")
    }

    /// `host:port`, for the probe that reads a refusal's sign-in schemes.
    pub(crate) fn address(&self) -> String {
        let rest = self
            .url
            .split_once("://")
            .map_or(&*self.url, |(_, rest)| rest);
        let host = rest.trim_end_matches('/');
        if host
            .rsplit_once(':')
            .is_some_and(|(_, port)| port.parse::<u16>().is_ok())
        {
            host.to_owned()
        } else {
            format!(
                "{host}:{}",
                if self.url.starts_with("https:") {
                    443
                } else {
                    80
                }
            )
        }
    }
}

impl Ctx {
    /// The proxy `url` goes through, or `None` for a direct connection.
    /// For one call, the environment chooses: `HTTPS_PROXY`, `NO_PROXY`, or
    /// `AGENT_CLI_CORE_PROXY` over a configured one.
    pub fn via(&self, url: &str) -> Result<Option<Via>> {
        self.core_via(url)
    }

    fn core_via(&self, url: &str) -> Result<Option<Via>> {
        let host = host_port(url);
        if let Some(raw) = self.config().core("proxy") {
            if raw.eq_ignore_ascii_case("none") {
                return Ok(None);
            }
            if self.exempt(host.as_ref())? {
                return Ok(None);
            }
            let (proxy, user) =
                split_proxy(&raw).map_err(|why| Failure::setup(format!("[core] proxy: {why}")))?;
            if user.is_some() {
                return Err(Failure::setup(
                    "[core] proxy holds a user or password, which would print wherever the proxy is named",
                )
                .hint("set proxy to http://HOST:PORT and the sign-in in proxy_user and proxy_password_env (or _cmd, _kv)")
                .into());
            }
            let user = self.config().core("proxy_user");
            let password = match &user {
                Some(_) => self.proxy_password()?,
                None => None,
            };
            return Ok(Some(Via {
                url: proxy,
                from: "[core] proxy".to_owned(),
                user,
                password,
            }));
        }
        let https = url.starts_with("https:");
        let names: &[&str] = if https {
            &["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"]
        } else {
            &["HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"]
        };
        let Some((name, raw)) = names
            .iter()
            .find_map(|name| self.env(name).map(|value| (*name, value)))
        else {
            return Ok(None);
        };
        if self.exempt(host.as_ref())? {
            return Ok(None);
        }
        let (proxy, user) =
            split_proxy(&raw).map_err(|why| Failure::setup(format!("{name}: {why}")))?;
        let (user, password) = match user {
            Some((user, password)) => (Some(user), password.map(Secret::new)),
            None => (None, None),
        };
        Ok(Some(Via {
            url: proxy,
            from: name.to_owned(),
            user,
            password,
        }))
    }

    /// True when `no_proxy` (`[core]`, else `NO_PROXY`) exempts the host.
    fn exempt(&self, host: Option<&(String, Option<u16>)>) -> Result<bool> {
        let Some((host, port)) = host else {
            return Ok(false);
        };
        let entries = match self
            .config()
            .merged()
            .and_then(|table| table.get("core"))
            .and_then(|core| core.get("no_proxy"))
        {
            Some(Value::Array(items)) => items
                .iter()
                .map(|item| {
                    item.as_str().map(str::to_owned).ok_or_else(|| {
                        Failure::setup("[core] no_proxy holds something other than names")
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
            Some(Value::String(list)) => list.split(',').map(str::to_owned).collect(),
            Some(_) => {
                return Err(Failure::setup("[core] no_proxy is not a list of names").into());
            }
            None => self
                .env("NO_PROXY")
                .or_else(|| self.env("no_proxy"))
                .map(|list| list.split(',').map(str::to_owned).collect())
                .unwrap_or_default(),
        };
        Ok(entries
            .iter()
            .any(|entry| no_proxy_matches(entry.trim(), host, *port)))
    }

    /// `[core] proxy_password` from wherever it is kept, resolved once a run.
    fn proxy_password(&self) -> Result<Option<Secret>> {
        let mut held = locked(&self.proxy_password);
        if let Some(password) = held.as_ref() {
            return Ok(password.clone());
        }
        let key = |name: &str| self.config().core(name);
        let credential = Credential::from_keys(
            "proxy_password",
            key("proxy_password"),
            key("proxy_password_env"),
            key("proxy_password_cmd"),
            key("proxy_password_kv"),
        )
        .map_err(|why| Failure::setup(format!("[core] {why}")))?;
        let password = credential.map(|c| c.resolve(self)).transpose()?;
        *held = Some(password.clone());
        Ok(password)
    }

    /// doctor's line for the proxy: which, from where, and what it exempts.
    pub(crate) fn proxy_detail(&self) -> Result<String> {
        let probe = "https://login.microsoftonline.com/";
        let via = self.via(probe)?;
        let exempt = match self.config().core("no_proxy") {
            Some(list) => format!("; no_proxy {list}"),
            None => match self
                .config()
                .merged()
                .and_then(|table| table.get("core")?.get("no_proxy")?.as_array().cloned())
            {
                Some(items) => {
                    let names: Vec<&str> = items.iter().filter_map(Value::as_str).collect();
                    format!("; no_proxy {}", names.join(","))
                }
                None => self
                    .env("NO_PROXY")
                    .or_else(|| self.env("no_proxy"))
                    .map(|list| format!("; NO_PROXY {list}"))
                    .unwrap_or_default(),
            },
        };
        Ok(match via {
            None if self.config().core("proxy").is_some() => {
                "none ([core] proxy = \"none\")".to_owned()
            }
            None => "none (no [core] proxy or HTTPS_PROXY)".to_owned(),
            Some(via) => {
                let signed = match &via.user {
                    Some(user) => format!(", signed in as {user}"),
                    None => String::new(),
                };
                format!("{} from {}{signed}{exempt}", via.url, via.from)
            }
        })
    }
}

/// A proxy URL split into `scheme://host:port` and its sign-in, if any.
#[allow(clippy::type_complexity)]
fn split_proxy(raw: &str) -> Result<(String, Option<(String, Option<String>)>), String> {
    let raw = raw.trim().trim_end_matches('/');
    let (scheme, rest) = match raw.split_once("://") {
        Some((scheme, rest)) => (scheme.to_ascii_lowercase(), rest),
        // curl's reading: a bare host:port is an http proxy.
        None => ("http".to_owned(), raw),
    };
    if !matches!(
        scheme.as_str(),
        "http" | "https" | "socks5" | "socks5h" | "socks4"
    ) {
        return Err(format!(
            "{raw:?} is not an http://, https:// or socks5:// proxy"
        ));
    }
    let (userinfo, host) = match rest.rsplit_once('@') {
        Some((userinfo, host)) => (Some(userinfo), host),
        None => (None, rest),
    };
    if host.is_empty() || host.contains(['/', '?', '#', ' ']) {
        return Err(format!("{raw:?} is not SCHEME://HOST:PORT"));
    }
    let user = userinfo.map(|userinfo| match userinfo.split_once(':') {
        Some((user, password)) => (decode(user), Some(decode(password))),
        None => (decode(userinfo), None),
    });
    Ok((format!("{scheme}://{host}"), user))
}

fn decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |at: usize| bytes.get(at).and_then(|b| (*b as char).to_digit(16));
        match (bytes[i], hex(i + 1), hex(i + 2)) {
            (b'%', Some(high), Some(low)) => {
                out.push(u8::try_from(high * 16 + low).unwrap_or(b'?'));
                i += 3;
            }
            (byte, ..) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The host (lowercase, IPv6 without brackets) and explicit port of a URL.
fn host_port(url: &str) -> Option<(String, Option<u16>)> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    if let Some(v6) = authority.strip_prefix('[') {
        let (host, after) = v6.split_once(']')?;
        let port = after.strip_prefix(':').and_then(|port| port.parse().ok());
        return Some((host.to_ascii_lowercase(), port));
    }
    match authority.rsplit_once(':') {
        Some((host, port)) => Some((host.to_ascii_lowercase(), port.parse().ok())),
        None => Some((authority.to_ascii_lowercase(), None)),
    }
}

/// curl's reading of one `NO_PROXY` entry: `*`, a name that covers its
/// subdomains (`contoso.example`, `.contoso.example`, `*.contoso.example`),
/// an address, a CIDR range, each with an optional `:port`.
fn no_proxy_matches(entry: &str, host: &str, port: Option<u16>) -> bool {
    let entry = entry.to_ascii_lowercase();
    if entry.is_empty() {
        return false;
    }
    if entry == "*" {
        return true;
    }
    if let Some((net, bits)) = entry.split_once('/') {
        return match (
            host.parse::<IpAddr>(),
            net.parse::<IpAddr>(),
            bits.parse::<u32>(),
        ) {
            (Ok(IpAddr::V4(ip)), Ok(IpAddr::V4(net)), Ok(bits)) if bits <= 32 => {
                let mask = u32::MAX.checked_shl(32 - bits).unwrap_or(0);
                u32::from(ip) & mask == u32::from(net) & mask
            }
            (Ok(IpAddr::V6(ip)), Ok(IpAddr::V6(net)), Ok(bits)) if bits <= 128 => {
                let mask = u128::MAX.checked_shl(128 - bits).unwrap_or(0);
                u128::from(ip) & mask == u128::from(net) & mask
            }
            _ => false,
        };
    }
    let (name, wanted_port) = match entry.trim_start_matches('[').rsplit_once(':') {
        Some((name, port)) if port.parse::<u16>().is_ok() && !name.contains(':') => {
            (name.trim_end_matches(']'), port.parse::<u16>().ok())
        }
        _ => (entry.trim_start_matches('[').trim_end_matches(']'), None),
    };
    if wanted_port.is_some() && wanted_port != port {
        return false;
    }
    let name = name.trim_start_matches("*.").trim_start_matches('.');
    host == name || host.ends_with(&format!(".{name}"))
}

/// The `Proxy-Authenticate` schemes a proxy offers, read by sending it one
/// `CONNECT` with no sign-in: ureq reports a refused `CONNECT` by its status
/// alone, and the scheme is what says whether a password can help at all.
/// Empty when the proxy does not say within a few seconds.
pub(crate) fn proxy_schemes(via: &Via, url: &str, timeout: Duration) -> Vec<String> {
    let target = url
        .split_once("://")
        .map(|(scheme, rest)| {
            let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
            let port = if scheme == "https" { 443 } else { 80 };
            if authority
                .rsplit_once(':')
                .is_some_and(|(_, p)| p.parse::<u16>().is_ok())
            {
                authority.to_owned()
            } else {
                format!("{authority}:{port}")
            }
        })
        .unwrap_or_default();
    let wait = timeout.min(Duration::from_secs(3));
    let read = || -> std::io::Result<String> {
        let address = via
            .address()
            .to_socket_addrs()?
            .next()
            .ok_or(std::io::ErrorKind::NotFound)?;
        let mut stream = TcpStream::connect_timeout(&address, wait)?;
        stream.set_read_timeout(Some(wait))?;
        write!(
            stream,
            "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n"
        )?;
        let mut head = Vec::new();
        let mut chunk = [0_u8; 2048];
        while head.len() < 16 * 1024 && !head.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = stream.read(&mut chunk)?;
            if n == 0 {
                break;
            }
            head.extend_from_slice(&chunk[..n]);
        }
        Ok(String::from_utf8_lossy(&head).into_owned())
    };
    read()
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("proxy-authenticate")
                .then(|| value.trim().to_owned())
        })
        .collect()
}

/// What to do about a proxy's `407`, from the schemes it offers and where
/// its sign-in came from.
pub(crate) fn proxy_refused(via: &Via, schemes: &[String]) -> String {
    let words: Vec<String> = schemes
        .iter()
        .filter_map(|scheme| scheme.split_whitespace().next())
        .map(str::to_ascii_lowercase)
        .collect();
    let basic = words.iter().any(|word| word == "basic");
    let windows = words
        .iter()
        .any(|word| matches!(word.as_str(), "negotiate" | "ntlm" | "kerberos"));
    if windows && !basic {
        return format!(
            "the proxy wants NTLM or Kerberos (Proxy-Authenticate: {}), which agent-cli does not speak: run px or cntlm on this machine and set [core] proxy to it, e.g. proxy = \"http://127.0.0.1:3128\"",
            schemes.join(", ")
        );
    }
    match (&via.user, via.from.as_str()) {
        (None, "[core] proxy") => "the proxy wants a sign-in: set [core] proxy_user and proxy_password_env (or proxy_password_cmd, proxy_password_kv)".to_owned(),
        (None, "--proxy") => "the proxy wants a sign-in, which --proxy never carries: set the proxy and its sign-in in [core] (proxy, proxy_user, proxy_password_env) and leave --proxy out".to_owned(),
        (None, from) => format!(
            "the proxy wants a sign-in: set [core] proxy = \"{}\", proxy_user and proxy_password_env (a user and password in {from}'s URL work too, but show wherever the environment is listed)",
            via.url
        ),
        (Some(user), "[core] proxy") => format!(
            "the proxy refused {user}: check where [core] proxy_password comes from (agent-cli doctor names it)"
        ),
        (Some(user), from) => format!("the proxy refused {user}: check the user and password in {from}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctx::Setup;
    use crate::testing::{FakeTransport, ctx};

    fn with(config: &str, env: &[(&str, &str)]) -> Ctx {
        let mut setup = Setup::fake(FakeTransport::default()).with_config(config);
        for (name, value) in env {
            setup = setup.with_env(name, value);
        }
        ctx(setup)
    }

    #[test]
    fn no_proxy_entries_read_as_curl_reads_them() {
        assert!(no_proxy_matches(
            "contoso.example",
            "api.contoso.example",
            None
        ));
        assert!(no_proxy_matches(
            ".contoso.example",
            "contoso.example",
            None
        ));
        assert!(no_proxy_matches(
            "*.contoso.example",
            "a.b.contoso.example",
            Some(443)
        ));
        assert!(!no_proxy_matches(
            "contoso.example",
            "notcontoso.example",
            None
        ));
        assert!(no_proxy_matches("10.0.0.0/8", "10.2.3.4", None));
        assert!(!no_proxy_matches("10.0.0.0/8", "11.2.3.4", None));
        assert!(no_proxy_matches("localhost:8080", "localhost", Some(8080)));
        assert!(!no_proxy_matches("localhost:8080", "localhost", Some(9090)));
        assert!(no_proxy_matches("::1", "::1", None));
        assert!(no_proxy_matches("*", "anything.example", None));
    }

    #[test]
    fn the_environment_names_the_proxy_and_no_proxy_exempts_a_host() {
        let ctx = with(
            "",
            &[
                (
                    "HTTPS_PROXY",
                    "http://agent:p%40ss@proxy.contoso.example:3128",
                ),
                ("NO_PROXY", "localhost,.internal.contoso.example"),
            ],
        );
        let via = ctx.via("https://api.contoso.example/x").unwrap().unwrap();
        assert_eq!(via.url, "http://proxy.contoso.example:3128");
        assert_eq!(via.from, "HTTPS_PROXY");
        assert_eq!(via.user.as_deref(), Some("agent"));
        assert_eq!(via.password.as_ref().map(Secret::expose), Some("p@ss"));
        assert_eq!(via.wire(), "http://agent:p%40ss@proxy.contoso.example:3128");
        for exempt in [
            "https://localhost:8443/",
            "https://db.internal.contoso.example/",
        ] {
            assert!(ctx.via(exempt).unwrap().is_none(), "{exempt}");
        }
        // HTTPS_PROXY is for https URLs only.
        assert!(ctx.via("http://api.contoso.example/").unwrap().is_none());
    }

    #[test]
    fn the_config_proxy_beats_the_environment_and_takes_its_sign_in_from_its_own_keys() {
        let ctx = with(
            "[core]\nproxy = \"http://proxy.contoso.example:8080\"\nproxy_user = \"agent\"\nproxy_password_env = \"PROXY_PASS\"\nno_proxy = [\"10.0.0.0/8\"]\n",
            &[
                ("HTTPS_PROXY", "http://other.example:1"),
                ("PROXY_PASS", "s3cret"),
            ],
        );
        let via = ctx.via("https://api.contoso.example/").unwrap().unwrap();
        assert_eq!(
            (via.url.as_str(), via.from.as_str()),
            ("http://proxy.contoso.example:8080", "[core] proxy")
        );
        assert_eq!(via.password.as_ref().map(Secret::expose), Some("s3cret"));
        assert!(ctx.via("https://10.1.2.3/").unwrap().is_none());
        let detail = ctx.proxy_detail().unwrap();
        assert!(
            detail.starts_with(
                "http://proxy.contoso.example:8080 from [core] proxy, signed in as agent"
            ),
            "{detail}"
        );
        assert!(!detail.contains("s3cret"));
    }

    #[test]
    fn a_proxy_url_holding_a_password_is_refused_where_it_would_print() {
        let ctx = with(
            "[core]\nproxy = \"http://agent:pw@proxy.contoso.example:8080\"\n",
            &[],
        );
        let error = ctx.via("https://a.example/").unwrap_err();
        assert!(format!("{error:#}").contains("holds a user or password"));
        let off = with(
            "[core]\nproxy = \"none\"\n",
            &[("HTTPS_PROXY", "http://p.example:1")],
        );
        assert!(off.via("https://a.example/").unwrap().is_none());
    }

    #[test]
    fn a_refusal_says_whether_a_password_can_help() {
        let via = |user: Option<&str>, from: &str| Via {
            url: "http://proxy.contoso.example:8080".into(),
            from: from.into(),
            user: user.map(str::to_owned),
            password: None,
        };
        assert!(
            proxy_refused(
                &via(None, "[core] proxy"),
                &["Basic realm=\"office\"".into()]
            )
            .contains("proxy_user")
        );
        assert!(
            proxy_refused(
                &via(None, "HTTPS_PROXY"),
                &["Negotiate".into(), "NTLM".into()]
            )
            .contains("px or cntlm")
        );
        assert!(
            proxy_refused(&via(Some("agent"), "[core] proxy"), &["Basic".into()])
                .contains("refused agent")
        );
    }
}
