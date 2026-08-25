//! In-memory, single-process fixed-window rate limits and peer source extraction.
//!
//! These guardrails are intentionally process-local because push-gateway does not
//! support multi-process production operation yet. Counters reset on restart.

use std::{
    collections::HashMap,
    net::{IpAddr, Ipv6Addr, SocketAddr},
    str::FromStr,
    sync::Arc,
};

use axum::http::HeaderMap;
use tokio::sync::Mutex;

use crate::time::unix_timestamp;

const DEFAULT_LIMITER_FAMILY_CAPACITY: usize = 100_000;

/// Independently bounded limiter families. Saturating one family cannot reset
/// or evict live protection in another family.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum RateLimitFamily {
    AuthSource,
    RegistrationSource,
    RegistrationRecipientSource,
    HookInvocationSource,
    HookInvocationHook,
    HookCreationRecipient,
}

#[derive(Clone, Debug)]
pub struct RateLimiters {
    inner: Arc<Mutex<RateLimitState>>,
    family_capacity: usize,
}

impl Default for RateLimiters {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(RateLimitState::default())),
            family_capacity: DEFAULT_LIMITER_FAMILY_CAPACITY,
        }
    }
}

#[derive(Default, Debug)]
struct RateLimitState {
    families: HashMap<RateLimitFamily, HashMap<String, FixedWindow>>,
}

#[derive(Clone, Copy, Debug)]
struct FixedWindow {
    started_at: i64,
    window_seconds: i64,
    count: u64,
}

impl RateLimiters {
    /// Attempts to consume one slot from a fixed-window rate limit.
    ///
    /// Returns `true` when the request is accepted. Expired windows are pruned on
    /// each call. Existing live windows remain enforceable when a family's key
    /// capacity is saturated; admission of a new key fails closed.
    pub(crate) async fn check(
        &self,
        family: RateLimitFamily,
        key: impl Into<String>,
        max_requests: u64,
        window_seconds: u64,
    ) -> bool {
        if max_requests == 0 || window_seconds == 0 {
            return true;
        }
        let now = unix_timestamp();
        let window_seconds = i64::try_from(window_seconds).unwrap_or(i64::MAX);
        let mut inner = self.inner.lock().await;
        let windows = inner.families.entry(family).or_default();
        windows.retain(|_, window| now < window.started_at.saturating_add(window.window_seconds));
        let key = key.into();
        if !windows.contains_key(&key) && self.family_capacity <= windows.len() {
            return false;
        }
        let window = windows.entry(key).or_insert(FixedWindow {
            started_at: now,
            window_seconds,
            count: 0,
        });
        if window.started_at.saturating_add(window_seconds) <= now {
            window.started_at = now;
            window.window_seconds = window_seconds;
            window.count = 0;
        }
        if window.count >= max_requests {
            return false;
        }
        window.count += 1;
        true
    }

    #[cfg(test)]
    fn with_family_capacity_for_tests(family_capacity: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(RateLimitState::default())),
            family_capacity,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrustedProxyCidrs(Vec<IpCidr>);

impl TrustedProxyCidrs {
    pub fn parse_csv(value: &str) -> Result<Self, String> {
        let mut cidrs = Vec::new();
        for part in value
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
        {
            cidrs.push(part.parse()?);
        }
        Ok(Self(cidrs))
    }

    pub fn is_trusted(&self, ip: IpAddr) -> bool {
        self.0.iter().any(|cidr| cidr.contains(ip))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum IpCidr {
    V4(u32, u8),
    V6(u128, u8),
}

impl FromStr for IpCidr {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (ip, prefix) = match value.split_once('/') {
            Some((ip, prefix)) => (ip, Some(prefix)),
            None => (value, None),
        };
        let ip: IpAddr = ip
            .parse()
            .map_err(|_| format!("invalid trusted proxy CIDR: {value}"))?;
        match ip {
            IpAddr::V4(addr) => {
                let prefix = parse_prefix(prefix, 32, value)?;
                let bits = u32::from(addr);
                Ok(Self::V4(bits & v4_mask(prefix), prefix))
            }
            IpAddr::V6(addr) => {
                let prefix = parse_prefix(prefix, 128, value)?;
                let bits = u128::from(addr);
                Ok(Self::V6(bits & v6_mask(prefix), prefix))
            }
        }
    }
}

impl IpCidr {
    fn contains(&self, ip: IpAddr) -> bool {
        match (self, ip) {
            (Self::V4(network, prefix), IpAddr::V4(addr)) => {
                (u32::from(addr) & v4_mask(*prefix)) == *network
            }
            (Self::V6(network, prefix), IpAddr::V6(addr)) => {
                (u128::from(addr) & v6_mask(*prefix)) == *network
            }
            _ => false,
        }
    }
}

fn parse_prefix(prefix: Option<&str>, default: u8, original: &str) -> Result<u8, String> {
    let Some(prefix) = prefix else {
        return Ok(default);
    };
    let parsed: u8 = prefix
        .parse()
        .map_err(|_| format!("invalid trusted proxy CIDR prefix: {original}"))?;
    if parsed > default {
        return Err(format!("invalid trusted proxy CIDR prefix: {original}"));
    }
    Ok(parsed)
}

fn v4_mask(prefix: u8) -> u32 {
    if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    }
}

fn v6_mask(prefix: u8) -> u128 {
    if prefix == 0 {
        0
    } else {
        u128::MAX << (128 - prefix)
    }
}

pub fn effective_source_prefix(
    direct_peer: Option<SocketAddr>,
    headers: &HeaderMap,
    trusted_proxies: Option<&TrustedProxyCidrs>,
) -> String {
    let Some(direct_ip) = direct_peer.map(|addr| addr.ip()) else {
        return normalize_source_prefix(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
    };
    let source_ip = if trusted_proxies.is_some_and(|proxies| proxies.is_trusted(direct_ip)) {
        forwarded_for(headers, trusted_proxies).unwrap_or(direct_ip)
    } else {
        direct_ip
    };
    normalize_source_prefix(source_ip)
}

pub fn normalize_source_prefix(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(addr) => addr.to_string(),
        IpAddr::V6(addr) => {
            let segments = addr.segments();
            Ipv6Addr::new(
                segments[0],
                segments[1],
                segments[2],
                segments[3],
                0,
                0,
                0,
                0,
            )
            .to_string()
                + "/64"
        }
    }
}

fn forwarded_for(
    headers: &HeaderMap,
    trusted_proxies: Option<&TrustedProxyCidrs>,
) -> Option<IpAddr> {
    let mut chain = Vec::new();
    if let Some(value) = headers.get("forwarded")
        && let Ok(value) = value.to_str()
    {
        for item in value.split(',') {
            for param in item.split(';') {
                if let Some((name, value)) = param.trim().split_once('=')
                    && name.trim().eq_ignore_ascii_case("for")
                    && let Some(ip) = parse_forwarded_ip(value.trim())
                {
                    chain.push(ip);
                }
            }
        }
    }
    if chain.is_empty()
        && let Some(value) = headers.get("x-forwarded-for")
        && let Ok(value) = value.to_str()
    {
        chain.extend(
            value
                .split(',')
                .filter_map(|ip| parse_forwarded_ip(ip.trim())),
        );
    }
    select_client_from_forwarded_chain(&chain, trusted_proxies)
}

fn select_client_from_forwarded_chain(
    chain: &[IpAddr],
    trusted_proxies: Option<&TrustedProxyCidrs>,
) -> Option<IpAddr> {
    let proxies = trusted_proxies?;
    chain
        .iter()
        .rev()
        .copied()
        .find(|ip| !proxies.is_trusted(*ip))
        .or_else(|| chain.first().copied())
}

fn parse_forwarded_ip(value: &str) -> Option<IpAddr> {
    let value = value.trim_matches('"').trim();
    if let Ok(ip) = value.parse() {
        return Some(ip);
    }
    if let Some(rest) = value.strip_prefix('[') {
        return rest.split(']').next()?.parse().ok();
    }
    value.split(':').next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    #[tokio::test]
    async fn saturated_family_does_not_evict_live_windows_or_affect_other_families() {
        let limiters = RateLimiters::with_family_capacity_for_tests(2);
        assert!(
            limiters
                .check(RateLimitFamily::RegistrationSource, "source-a", 1, 60)
                .await
        );
        assert!(
            limiters
                .check(RateLimitFamily::RegistrationSource, "source-b", 1, 60)
                .await
        );
        assert!(
            !limiters
                .check(RateLimitFamily::RegistrationSource, "source-c", 1, 60)
                .await
        );
        assert!(
            !limiters
                .check(RateLimitFamily::RegistrationSource, "source-a", 1, 60)
                .await
        );
        assert!(
            limiters
                .check(RateLimitFamily::AuthSource, "source-c", 1, 60)
                .await
        );
    }

    #[test]
    fn normalizes_ipv6_to_64() {
        assert_eq!(
            normalize_source_prefix("2001:db8:abcd:12:3456:789a::1".parse().unwrap()),
            "2001:db8:abcd:12::/64"
        );
    }

    #[test]
    fn trusted_proxy_uses_forwarded_header() {
        let proxies = TrustedProxyCidrs::parse_csv("10.0.0.0/8").unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_static("198.51.100.9"));
        assert_eq!(
            effective_source_prefix(
                Some("10.1.2.3:443".parse().unwrap()),
                &headers,
                Some(&proxies)
            ),
            "198.51.100.9"
        );
    }

    #[test]
    fn untrusted_peer_ignores_forwarded_header() {
        let proxies = TrustedProxyCidrs::parse_csv("10.0.0.0/8").unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_static("198.51.100.9"));
        assert_eq!(
            effective_source_prefix(
                Some("203.0.113.7:443".parse().unwrap()),
                &headers,
                Some(&proxies)
            ),
            "203.0.113.7"
        );
    }

    #[test]
    fn trusted_proxy_uses_rightmost_untrusted_forwarded_for_entry() {
        let proxies = TrustedProxyCidrs::parse_csv("10.0.0.0/8").unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static("198.51.100.9, 203.0.113.7"),
        );
        assert_eq!(
            effective_source_prefix(
                Some("10.1.2.3:443".parse().unwrap()),
                &headers,
                Some(&proxies)
            ),
            "203.0.113.7"
        );
    }
}
