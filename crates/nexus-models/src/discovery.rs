//! Provider endpoint probing and model discovery for the interactive
//! `/connect` flows. Read-only: never installs, starts, or downloads anything.

use serde::Serialize;
use std::time::{Duration, Instant};

/// A model reported by a provider endpoint.
#[derive(Debug, Clone, Serialize)]
pub struct DiscoveredModel {
    pub id: String,
    /// On-disk size in bytes (Ollama reports this; OpenAI-style listings don't).
    pub size_bytes: Option<u64>,
    pub family: Option<String>,
    pub parameter_size: Option<String>,
    pub quantization: Option<String>,
    /// Human-readable name when the provider reports one (Codex plan models).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Provider-supplied description of what the model is for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Why an endpoint probe failed, classified so the UI can offer the right
/// next action instead of a generic "failed".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeFailure {
    ConnectionRefused,
    DnsFailure,
    TlsFailure,
    Timeout,
    InvalidCredentials,
    PermissionDenied,
    RateLimited,
    UnsupportedEndpoint,
    MalformedResponse,
    NoModels,
    InvalidUrl,
    Other,
}

impl ProbeFailure {
    pub fn describe(&self) -> &'static str {
        match self {
            ProbeFailure::ConnectionRefused => "connection refused (is the server running?)",
            ProbeFailure::DnsFailure => "DNS lookup failed (check the host name)",
            ProbeFailure::TlsFailure => "TLS handshake failed (certificate or protocol problem)",
            ProbeFailure::Timeout => "the endpoint did not answer in time",
            ProbeFailure::InvalidCredentials => "the endpoint rejected the credentials (401)",
            ProbeFailure::PermissionDenied => "the endpoint denied access (403)",
            ProbeFailure::RateLimited => "the endpoint is rate-limiting requests (429)",
            ProbeFailure::UnsupportedEndpoint => {
                "the path is not served here (wrong base URL or protocol?)"
            }
            ProbeFailure::MalformedResponse => "the endpoint answered with unexpected JSON",
            ProbeFailure::NoModels => "the endpoint is up but reports no models",
            ProbeFailure::InvalidUrl => "the URL is not valid",
            ProbeFailure::Other => "the request failed",
        }
    }
}

/// Probe error carrying the classification and a redaction-safe detail line.
#[derive(Debug, Clone)]
pub struct ProbeError {
    pub failure: ProbeFailure,
    pub detail: String,
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.failure.describe(), self.detail)
    }
}

/// Outcome of a successful probe.
#[derive(Debug, Clone)]
pub struct ProbeOutcome {
    pub models: Vec<DiscoveredModel>,
    pub latency_ms: u64,
}

fn classify_reqwest(e: &reqwest::Error) -> ProbeFailure {
    if e.is_timeout() {
        return ProbeFailure::Timeout;
    }
    if e.is_builder() {
        return ProbeFailure::InvalidUrl;
    }
    if e.is_connect() {
        let text = format!("{e:?}").to_lowercase();
        if text.contains("dns") || text.contains("resolve") {
            return ProbeFailure::DnsFailure;
        }
        if text.contains("tls") || text.contains("certificate") || text.contains("ssl") {
            return ProbeFailure::TlsFailure;
        }
        return ProbeFailure::ConnectionRefused;
    }
    ProbeFailure::Other
}

fn classify_status(status: reqwest::StatusCode) -> ProbeFailure {
    match status.as_u16() {
        401 => ProbeFailure::InvalidCredentials,
        403 => ProbeFailure::PermissionDenied,
        404 | 405 => ProbeFailure::UnsupportedEndpoint,
        429 => ProbeFailure::RateLimited,
        _ => ProbeFailure::Other,
    }
}

fn client(timeout: Duration, tls_verify: bool) -> Result<reqwest::Client, ProbeError> {
    reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(timeout.min(Duration::from_secs(5)))
        .danger_accept_invalid_certs(!tls_verify)
        .build()
        .map_err(|e| ProbeError {
            failure: ProbeFailure::Other,
            detail: format!("http client: {e}"),
        })
}

/// Validate a user-supplied base URL: http(s) only, host present. Applied
/// before any custom-endpoint probe so typos fail fast with a clear message.
pub fn validate_base_url(url: &str) -> Result<url::Url, ProbeError> {
    let parsed = url::Url::parse(url).map_err(|e| ProbeError {
        failure: ProbeFailure::InvalidUrl,
        detail: e.to_string(),
    })?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(ProbeError {
            failure: ProbeFailure::InvalidUrl,
            detail: format!(
                "scheme `{}` not supported (use http/https)",
                parsed.scheme()
            ),
        });
    }
    if parsed.host_str().is_none() {
        return Err(ProbeError {
            failure: ProbeFailure::InvalidUrl,
            detail: "missing host".into(),
        });
    }
    Ok(parsed)
}

/// List models from an Ollama server (`GET {base}/api/tags`).
pub async fn list_ollama_models(
    base_url: &str,
    timeout: Duration,
) -> Result<ProbeOutcome, ProbeError> {
    list_ollama_models_with_tls(base_url, timeout, true).await
}

/// List models from an Ollama server with an explicit TLS verification choice.
/// Certificate verification remains enabled unless the operator deliberately
/// disables it for a custom endpoint.
pub async fn list_ollama_models_with_tls(
    base_url: &str,
    timeout: Duration,
    tls_verify: bool,
) -> Result<ProbeOutcome, ProbeError> {
    let base = validate_base_url(base_url)?;
    let url = format!("{}/api/tags", base.as_str().trim_end_matches('/'));
    let started = Instant::now();
    let resp = client(timeout, tls_verify)?
        .get(&url)
        .send()
        .await
        .map_err(|e| ProbeError {
            failure: classify_reqwest(&e),
            detail: format!("GET /api/tags: {e}"),
        })?;
    let status = resp.status();
    if !status.is_success() {
        return Err(ProbeError {
            failure: classify_status(status),
            detail: format!("GET /api/tags returned {status}"),
        });
    }
    let v: serde_json::Value = resp.json().await.map_err(|_| ProbeError {
        failure: ProbeFailure::MalformedResponse,
        detail: "response was not JSON".into(),
    })?;
    let Some(entries) = v.get("models").and_then(|m| m.as_array()) else {
        return Err(ProbeError {
            failure: ProbeFailure::MalformedResponse,
            detail: "no `models` array — this is not an Ollama endpoint".into(),
        });
    };
    let models = entries
        .iter()
        .filter_map(|m| {
            let details = m.get("details");
            Some(DiscoveredModel {
                id: m.get("name")?.as_str()?.to_string(),
                size_bytes: m.get("size").and_then(|s| s.as_u64()),
                family: details
                    .and_then(|d| d.get("family"))
                    .and_then(|f| f.as_str())
                    .map(String::from),
                parameter_size: details
                    .and_then(|d| d.get("parameter_size"))
                    .and_then(|p| p.as_str())
                    .map(String::from),
                quantization: details
                    .and_then(|d| d.get("quantization_level"))
                    .and_then(|q| q.as_str())
                    .map(String::from),
                display_name: None,
                description: None,
            })
        })
        .collect();
    Ok(ProbeOutcome {
        models,
        latency_ms: started.elapsed().as_millis() as u64,
    })
}

/// List models from an OpenAI-compatible server (`GET {base}/models`).
/// `base_url` should already include the `/v1` segment where applicable.
pub async fn list_openai_models(
    base_url: &str,
    api_key: Option<&str>,
    timeout: Duration,
) -> Result<ProbeOutcome, ProbeError> {
    list_openai_models_with_tls(base_url, api_key, timeout, true).await
}

/// List models from an OpenAI-compatible server with an explicit TLS
/// verification choice.
pub async fn list_openai_models_with_tls(
    base_url: &str,
    api_key: Option<&str>,
    timeout: Duration,
    tls_verify: bool,
) -> Result<ProbeOutcome, ProbeError> {
    let base = validate_base_url(base_url)?;
    let url = format!("{}/models", base.as_str().trim_end_matches('/'));
    let started = Instant::now();
    let mut req = client(timeout, tls_verify)?.get(&url);
    if let Some(key) = api_key.filter(|k| !k.is_empty()) {
        req = req.bearer_auth(key);
    }
    let resp = req.send().await.map_err(|e| ProbeError {
        failure: classify_reqwest(&e),
        detail: format!("GET /models: {e}"),
    })?;
    let status = resp.status();
    if !status.is_success() {
        return Err(ProbeError {
            failure: classify_status(status),
            detail: format!("GET /models returned {status}"),
        });
    }
    let v: serde_json::Value = resp.json().await.map_err(|_| ProbeError {
        failure: ProbeFailure::MalformedResponse,
        detail: "response was not JSON".into(),
    })?;
    let Some(entries) = v.get("data").and_then(|d| d.as_array()) else {
        return Err(ProbeError {
            failure: ProbeFailure::MalformedResponse,
            detail: "no `data` array — this is not an OpenAI-style /models endpoint".into(),
        });
    };
    let models = entries
        .iter()
        .filter_map(|m| {
            Some(DiscoveredModel {
                id: m.get("id")?.as_str()?.to_string(),
                size_bytes: None,
                family: None,
                parameter_size: None,
                quantization: None,
                display_name: None,
                description: None,
            })
        })
        .collect();
    Ok(ProbeOutcome {
        models,
        latency_ms: started.elapsed().as_millis() as u64,
    })
}

/// Human-readable size like `4.7 GB`.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn ollama_listing_parses_details() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/tags"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "models": [{
                    "name": "qwen3:4b",
                    "size": 2_700_000_000u64,
                    "details": {"family": "qwen3", "parameter_size": "4B", "quantization_level": "Q4_K_M"}
                }]
            })))
            .mount(&server)
            .await;
        let out = list_ollama_models(&server.uri(), Duration::from_secs(2))
            .await
            .expect("probe");
        assert_eq!(out.models.len(), 1);
        assert_eq!(out.models[0].id, "qwen3:4b");
        assert_eq!(out.models[0].parameter_size.as_deref(), Some("4B"));
    }

    #[tokio::test]
    async fn non_ollama_json_is_malformed_not_models() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/tags"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"status": "UP"})),
            )
            .mount(&server)
            .await;
        let err = list_ollama_models(&server.uri(), Duration::from_secs(2))
            .await
            .expect_err("must fail");
        assert_eq!(err.failure, ProbeFailure::MalformedResponse);
    }

    #[tokio::test]
    async fn openai_listing_classifies_auth_errors() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let err = list_openai_models(
            &format!("{}/v1", server.uri()),
            None,
            Duration::from_secs(2),
        )
        .await
        .expect_err("must fail");
        assert_eq!(err.failure, ProbeFailure::InvalidCredentials);
    }

    #[tokio::test]
    async fn connection_refused_is_classified() {
        // Port 1 is essentially guaranteed closed.
        let err = list_openai_models("http://127.0.0.1:1/v1", None, Duration::from_secs(2))
            .await
            .expect_err("must fail");
        assert!(matches!(
            err.failure,
            ProbeFailure::ConnectionRefused | ProbeFailure::Other | ProbeFailure::Timeout
        ));
    }

    #[test]
    fn url_validation_rejects_bad_schemes() {
        assert!(validate_base_url("ftp://x").is_err());
        assert!(validate_base_url("not a url").is_err());
        assert!(validate_base_url("http://127.0.0.1:8080/v1").is_ok());
    }

    #[test]
    fn human_size_formats() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2_700_000_000), "2.5 GB");
    }
}
