//! Reading a provider's "not right now" off the wire.
//!
//! Every metered provider says the same thing in a different dialect: OpenAI
//! and the Responses backend use `x-ratelimit-reset-*`, Anthropic uses
//! `anthropic-ratelimit-*-reset`, and everyone honors `Retry-After`. What they
//! agree on is the status code.
//!
//! The one rule here is that an absent field stays absent. A quota the
//! provider did not state is not something to estimate: the operator acts on
//! these numbers, and a plausible invention is worse than an honest gap.

use nexus_core::NexusError;
use reqwest::header::HeaderMap;

/// Whether a status means "wait", as opposed to "that request was wrong".
///
/// 429 is the explicit one. 529 is Anthropic's overload signal, which is also
/// a wait rather than a fault. 5xx generally is *not* included: a server error
/// is not a quota, and treating it as one would make the runtime sit out a
/// window that was never imposed.
pub fn is_rate_limit(status: u16) -> bool {
    matches!(status, 429 | 529)
}

/// Seconds to wait, if any header said so.
///
/// `Retry-After` may be a delay in seconds or an HTTP date; only the numeric
/// form is read, because the date form needs a clock comparison this layer has
/// no business making. The reset headers are tried next, in the dialects the
/// supported providers actually send.
fn retry_after_secs(headers: &HeaderMap) -> Option<u64> {
    const KEYS: [&str; 5] = [
        "retry-after",
        "x-ratelimit-reset-requests",
        "x-ratelimit-reset-tokens",
        "anthropic-ratelimit-requests-reset",
        "anthropic-ratelimit-tokens-reset",
    ];
    KEYS.iter().find_map(|key| {
        let raw = headers.get(*key)?.to_str().ok()?.trim().to_string();
        parse_duration_secs(&raw)
    })
}

/// `"30"`, `"30s"`, `"1m30s"`, `"1.5s"` — the forms these providers send.
///
/// Returns `None` for anything else, including HTTP dates, rather than
/// guessing at a number.
fn parse_duration_secs(raw: &str) -> Option<u64> {
    if let Ok(seconds) = raw.parse::<f64>() {
        return (seconds.is_finite() && seconds >= 0.0).then_some(seconds.ceil() as u64);
    }
    let mut total = 0f64;
    let mut number = String::new();
    let mut saw_unit = false;
    for ch in raw.chars() {
        if ch.is_ascii_digit() || ch == '.' {
            number.push(ch);
            continue;
        }
        let value: f64 = number.parse().ok()?;
        number.clear();
        total += match ch {
            'h' => value * 3600.0,
            'm' => value * 60.0,
            's' => value,
            _ => return None,
        };
        saw_unit = true;
    }
    (saw_unit && number.is_empty() && total.is_finite()).then_some(total.ceil() as u64)
}

/// The reset instant a provider stated, verbatim. Never synthesized.
fn reset_at(headers: &HeaderMap) -> Option<String> {
    const KEYS: [&str; 3] = [
        "x-ratelimit-reset",
        "anthropic-ratelimit-unified-reset",
        "x-ratelimit-reset-tokens",
    ];
    KEYS.iter().find_map(|key| {
        let raw = headers.get(*key)?.to_str().ok()?.trim();
        (!raw.is_empty()).then(|| raw.to_string())
    })
}

/// Which quota was hit, as far as the headers reveal.
///
/// `unknown` is a real answer here: several providers return 429 with nothing
/// but the status, and claiming to know which limit it was would be a guess.
fn limit_kind(headers: &HeaderMap, body: &str) -> String {
    let exhausted = |key: &str| {
        headers
            .get(key)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u64>().ok())
            .is_some_and(|remaining| remaining == 0)
    };
    if exhausted("x-ratelimit-remaining-tokens")
        || exhausted("anthropic-ratelimit-tokens-remaining")
    {
        return "tokens".into();
    }
    if exhausted("x-ratelimit-remaining-requests")
        || exhausted("anthropic-ratelimit-requests-remaining")
    {
        return "requests".into();
    }
    let body = body.to_ascii_lowercase();
    if body.contains("usage limit") || body.contains("plan") && body.contains("limit") {
        return "plan_window".into();
    }
    "unknown".into()
}

/// Build the typed limit error for a refused request.
pub fn error(provider: &str, headers: &HeaderMap, status: u16, body: &str) -> NexusError {
    let retry_after_secs = retry_after_secs(headers);
    let reset_at = reset_at(headers);
    let wait = match (retry_after_secs, reset_at.as_deref()) {
        (Some(secs), _) => format!(" — retry in {secs}s"),
        (None, Some(reset)) => format!(" — resets at {reset}"),
        // Said nothing about when. The message says so rather than implying
        // the wait is short.
        (None, None) => " — the provider did not say when it resets".to_string(),
    };
    NexusError::ProviderLimit {
        provider: provider.to_string(),
        kind: limit_kind(headers, body),
        retry_after_secs,
        reset_at,
        message: format!("HTTP {status}{wait}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderName, HeaderValue};

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (key, value) in pairs {
            map.insert(
                HeaderName::from_bytes(key.as_bytes()).expect("header name"),
                HeaderValue::from_str(value).expect("header value"),
            );
        }
        map
    }

    #[test]
    fn a_stated_delay_is_read_in_every_dialect() {
        for (key, raw, expected) in [
            ("retry-after", "30", 30u64),
            ("retry-after", "1.2", 2),
            ("x-ratelimit-reset-requests", "1m30s", 90),
            ("anthropic-ratelimit-tokens-reset", "2h", 7200),
        ] {
            let error = error("openai", &headers(&[(key, raw)]), 429, "");
            let NexusError::ProviderLimit {
                retry_after_secs, ..
            } = error
            else {
                panic!("expected a provider limit");
            };
            assert_eq!(retry_after_secs, Some(expected), "{key}={raw}");
        }
    }

    #[test]
    fn an_unstated_reset_is_reported_as_unstated_not_estimated() {
        // The operator plans around this number. A plausible invention is
        // worse than an admitted gap.
        let error = error("codex", &headers(&[]), 429, "rate limited");
        let NexusError::ProviderLimit {
            retry_after_secs,
            reset_at,
            message,
            ..
        } = error
        else {
            panic!("expected a provider limit");
        };
        assert_eq!(retry_after_secs, None);
        assert_eq!(reset_at, None);
        assert!(message.contains("did not say"), "{message}");
    }

    #[test]
    fn an_http_date_is_not_mistaken_for_a_delay() {
        let error = error(
            "openai",
            &headers(&[("retry-after", "Wed, 21 Oct 2026 07:28:00 GMT")]),
            429,
            "",
        );
        let NexusError::ProviderLimit {
            retry_after_secs, ..
        } = error
        else {
            panic!("expected a provider limit");
        };
        assert_eq!(retry_after_secs, None);
    }

    #[test]
    fn the_exhausted_quota_is_named_when_the_headers_reveal_it() {
        let tokens = error(
            "anthropic",
            &headers(&[("anthropic-ratelimit-tokens-remaining", "0")]),
            429,
            "",
        );
        let NexusError::ProviderLimit { kind, .. } = tokens else {
            panic!("expected a provider limit");
        };
        assert_eq!(kind, "tokens");

        let unknown = error("openai", &headers(&[]), 429, "");
        let NexusError::ProviderLimit { kind, .. } = unknown else {
            panic!("expected a provider limit");
        };
        assert_eq!(kind, "unknown");
    }

    #[test]
    fn only_waiting_statuses_count_as_limits() {
        assert!(is_rate_limit(429));
        // Anthropic's overload signal is also a wait.
        assert!(is_rate_limit(529));
        // A server fault is not a quota; sitting out a window nobody imposed
        // would be worse than failing fast.
        assert!(!is_rate_limit(500));
        assert!(!is_rate_limit(503));
        assert!(!is_rate_limit(401));
    }
}
