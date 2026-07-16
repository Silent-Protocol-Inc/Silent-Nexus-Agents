//! SSRF and network-destination protection.
//!
//! Every outbound URL is validated **before and after DNS resolution**:
//! * scheme must be http/https (no file:, ftp:, gopher:, …);
//! * no credentials embedded in the URL;
//! * host checked against config allowlist/denylist;
//! * every resolved IP must be public: loopback, RFC1918, link-local,
//!   CGNAT, unique-local and multicast ranges are refused (unless loopback is
//!   explicitly enabled for local-service testing);
//! * the validated IP is **pinned** for the actual connection, so a DNS
//!   rebind between check and use cannot redirect the request;
//! * redirects are followed manually, re-validating every hop.

use nexus_core::config::WebConfig;
use nexus_core::{NexusError, Result};
use std::net::{IpAddr, SocketAddr};
use url::Url;

/// A URL that passed validation, with its pinned socket address.
#[derive(Debug, Clone)]
pub struct ValidatedUrl {
    pub url: Url,
    pub host: String,
    pub addr: SocketAddr,
}

/// Cloud metadata endpoints that are always refused, even by IP literal.
const METADATA_HOSTS: &[&str] = &[
    "169.254.169.254",
    "metadata.google.internal",
    "metadata.goog",
    "100.100.100.200", // Alibaba
];

pub fn ip_is_public(ip: &IpAddr, allow_loopback: bool) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            if v4.is_loopback() {
                return allow_loopback;
            }
            !(v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_multicast()
                // CGNAT 100.64.0.0/10
                || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xC0) == 64)
                // Benchmarking 198.18.0.0/15
                || (v4.octets()[0] == 198 && (v4.octets()[1] & 0xFE) == 18))
        }
        IpAddr::V6(v6) => {
            if v6.is_loopback() {
                return allow_loopback;
            }
            if v6.is_unspecified() || v6.is_multicast() {
                return false;
            }
            let segs = v6.segments();
            // Unique-local fc00::/7
            if (segs[0] & 0xFE00) == 0xFC00 {
                return false;
            }
            // Link-local fe80::/10
            if (segs[0] & 0xFFC0) == 0xFE80 {
                return false;
            }
            // IPv4-mapped: validate the embedded IPv4.
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return ip_is_public(&IpAddr::V4(mapped), allow_loopback);
            }
            true
        }
    }
}

fn host_matches(pattern: &str, host: &str) -> bool {
    if let Some(suffix) = pattern.strip_prefix("*.") {
        host == suffix || host.ends_with(&format!(".{suffix}"))
    } else {
        host == pattern
    }
}

/// Validate a URL string against config and DNS, returning the pinned target.
pub async fn validate(raw_url: &str, config: &WebConfig) -> Result<ValidatedUrl> {
    if !config.enabled {
        return Err(NexusError::NetworkBlocked(
            "web access is disabled in configuration".into(),
        ));
    }
    let url =
        Url::parse(raw_url).map_err(|e| NexusError::NetworkBlocked(format!("invalid URL: {e}")))?;
    match url.scheme() {
        "http" | "https" => {}
        other => {
            return Err(NexusError::NetworkBlocked(format!(
                "scheme `{other}` is not allowed (http/https only)"
            )))
        }
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(NexusError::NetworkBlocked(
            "credential-bearing URLs are not allowed".into(),
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| NexusError::NetworkBlocked("URL has no host".into()))?
        .to_lowercase();

    if METADATA_HOSTS.iter().any(|m| host_matches(m, &host)) {
        return Err(NexusError::NetworkBlocked(format!(
            "`{host}` is a cloud metadata endpoint"
        )));
    }
    if config.denylist.iter().any(|p| host_matches(p, &host)) {
        return Err(NexusError::NetworkBlocked(format!(
            "`{host}` is on the configured denylist"
        )));
    }
    let allowlisted = config.allowlist.iter().any(|p| host_matches(p, &host));

    let port = url
        .port_or_known_default()
        .ok_or_else(|| NexusError::NetworkBlocked("cannot determine port".into()))?;

    // Resolve and validate every address; pin the first valid one.
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|e| NexusError::NetworkBlocked(format!("DNS lookup for `{host}` failed: {e}")))?
        .collect();
    if addrs.is_empty() {
        return Err(NexusError::NetworkBlocked(format!(
            "`{host}` resolved to no addresses"
        )));
    }
    let allow_loopback = config.allow_loopback || (allowlisted && is_loopback_host(&host));
    for addr in &addrs {
        if !ip_is_public(&addr.ip(), allow_loopback) {
            return Err(NexusError::NetworkBlocked(format!(
                "`{host}` resolves to non-public address {} (private/link-local/metadata ranges are blocked)",
                addr.ip()
            )));
        }
    }
    Ok(ValidatedUrl {
        host,
        addr: addrs[0],
        url,
    })
}

fn is_loopback_host(host: &str) -> bool {
    host == "localhost" || host == "127.0.0.1" || host == "::1"
}

/// Build a reqwest client pinned to the validated address, with redirects
/// disabled (callers follow redirects manually via [`validate`] per hop).
pub fn pinned_client(v: &ValidatedUrl, timeout_secs: u64) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .resolve(&v.host, v.addr)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .connect_timeout(std::time::Duration::from_secs(10))
        .user_agent(format!(
            "SilentNexus/{} (+https://silentprotocol.top)",
            nexus_core::brand::VERSION
        ))
        .build()
        .map_err(|e| NexusError::other(format!("client build: {e}")))
}

/// Follow up to `max_redirects` redirects, validating each hop. Returns the
/// final response.
pub async fn get_with_redirects(
    raw_url: &str,
    config: &WebConfig,
    max_redirects: usize,
    head_only: bool,
) -> Result<(reqwest::Response, ValidatedUrl)> {
    let mut current = raw_url.to_string();
    for _hop in 0..=max_redirects {
        let validated = validate(&current, config).await?;
        let client = pinned_client(&validated, config.timeout_secs)?;
        let req = if head_only {
            client.head(validated.url.clone())
        } else {
            client.get(validated.url.clone())
        };
        let response = req.send().await.map_err(|e| {
            NexusError::NetworkBlocked(format!("request to `{current}` failed: {e}"))
        })?;
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| {
                    NexusError::NetworkBlocked("redirect without Location header".into())
                })?;
            // Resolve relative redirects against the current URL.
            current = validated
                .url
                .join(location)
                .map_err(|e| NexusError::NetworkBlocked(format!("bad redirect target: {e}")))?
                .to_string();
            continue;
        }
        return Ok((response, validated));
    }
    Err(NexusError::NetworkBlocked(format!(
        "too many redirects (>{max_redirects}) fetching `{raw_url}`"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> WebConfig {
        WebConfig::default()
    }

    #[tokio::test]
    async fn blocks_unsafe_schemes() {
        for u in [
            "file:///etc/passwd",
            "ftp://example.com/x",
            "gopher://example.com",
        ] {
            assert!(matches!(
                validate(u, &cfg()).await,
                Err(NexusError::NetworkBlocked(_))
            ));
        }
    }

    #[tokio::test]
    async fn blocks_credentials_in_url() {
        assert!(validate("https://user:pass@example.com/", &cfg())
            .await
            .is_err());
    }

    #[tokio::test]
    async fn blocks_loopback_and_private_literals() {
        for u in [
            "http://127.0.0.1/x",
            "http://localhost:8080/",
            "http://10.0.0.5/",
            "http://192.168.1.1/",
            "http://172.16.0.1/",
            "http://169.254.169.254/latest/meta-data/",
            "http://[::1]/",
            "http://100.64.0.1/",
        ] {
            let out = validate(u, &cfg()).await;
            assert!(out.is_err(), "{u} should be blocked");
        }
    }

    #[tokio::test]
    async fn allows_loopback_when_enabled() {
        let mut config = cfg();
        config.allow_loopback = true;
        let out = validate("http://127.0.0.1:9/", &config).await;
        assert!(out.is_ok());
    }

    #[tokio::test]
    async fn denylist_blocks_host() {
        let mut config = cfg();
        config.denylist = vec!["*.blocked.example".into()];
        let out = validate("https://api.blocked.example/x", &config).await;
        assert!(matches!(out, Err(NexusError::NetworkBlocked(m)) if m.contains("denylist")));
    }

    #[test]
    fn ip_classification() {
        let public: IpAddr = "93.184.216.34".parse().expect("ip");
        assert!(ip_is_public(&public, false));
        let mapped: IpAddr = "::ffff:10.0.0.1".parse().expect("ip");
        assert!(!ip_is_public(&mapped, false));
        let ula: IpAddr = "fd00::1".parse().expect("ip");
        assert!(!ip_is_public(&ula, false));
    }

    #[test]
    fn wildcard_host_matching() {
        assert!(host_matches("*.example.com", "api.example.com"));
        assert!(host_matches("*.example.com", "example.com"));
        assert!(!host_matches("*.example.com", "evilexample.com"));
    }
}
