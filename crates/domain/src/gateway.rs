//! Shared gateway endpoint vocabulary.

use std::fmt;

/// Canonical client-reachable API URL for an LNv2 gateway.
#[derive(
    Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize,
)]
#[serde(try_from = "String", into = "String")]
pub struct GatewayApiUrl(String);

impl GatewayApiUrl {
    /// Maximum encoded URL length.
    pub const MAX_BYTES: usize = 2_048;

    /// Borrow the canonical URL.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for GatewayApiUrl {
    type Error = InvalidGatewayApiUrl;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl TryFrom<&str> for GatewayApiUrl {
    type Error = InvalidGatewayApiUrl;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        if value.len() > Self::MAX_BYTES {
            return Err(InvalidGatewayApiUrl);
        }
        let url = url::Url::parse(value).map_err(|_| InvalidGatewayApiUrl)?;
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(InvalidGatewayApiUrl);
        }
        match url.scheme() {
            "https" => validate_public_gateway_host(&url)?,
            "iroh" => {
                let endpoint = url.host_str().ok_or(InvalidGatewayApiUrl)?;
                if url.port().is_some()
                    || !matches!(url.path(), "" | "/")
                    || endpoint.len() != 64
                    || hex::decode(endpoint).map_or(true, |bytes| bytes.len() != 32)
                {
                    return Err(InvalidGatewayApiUrl);
                }
            }
            _ => return Err(InvalidGatewayApiUrl),
        }
        let canonical = url.to_string();
        if canonical.len() > Self::MAX_BYTES {
            return Err(InvalidGatewayApiUrl);
        }
        Ok(Self(canonical))
    }
}

impl From<GatewayApiUrl> for String {
    fn from(value: GatewayApiUrl) -> Self {
        value.0
    }
}

impl fmt::Display for GatewayApiUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A gateway API URL is malformed, unsafe, or not a supported transport.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("invalid gateway API URL")]
pub struct InvalidGatewayApiUrl;

fn validate_public_gateway_host(url: &url::Url) -> Result<(), InvalidGatewayApiUrl> {
    match url.host() {
        Some(url::Host::Ipv4(ip)) => {
            if ip.is_loopback()
                || ip.is_link_local()
                || ip.is_private()
                || ip.is_unspecified()
                || ip.is_multicast()
                || ip.is_broadcast()
            {
                return Err(InvalidGatewayApiUrl);
            }
        }
        Some(url::Host::Ipv6(ip)) => {
            if let Some(mapped) = ip.to_ipv4_mapped() {
                if mapped.is_loopback()
                    || mapped.is_link_local()
                    || mapped.is_private()
                    || mapped.is_unspecified()
                    || mapped.is_multicast()
                    || mapped.is_broadcast()
                {
                    return Err(InvalidGatewayApiUrl);
                }
            } else if ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || (ip.segments()[0] & 0xfe00) == 0xfc00
                || (ip.segments()[0] & 0xffc0) == 0xfe80
            {
                return Err(InvalidGatewayApiUrl);
            }
        }
        Some(url::Host::Domain(domain)) => {
            let domain = domain.strip_suffix('.').unwrap_or(domain);
            if domain.eq_ignore_ascii_case("localhost")
                || domain.to_ascii_lowercase().ends_with(".localhost")
                || !domain.contains('.')
            {
                return Err(InvalidGatewayApiUrl);
            }
        }
        None => return Err(InvalidGatewayApiUrl),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::GatewayApiUrl;

    #[test]
    fn canonical_and_client_reachable() {
        assert_eq!(
            GatewayApiUrl::try_from("https://gateway.example/api")
                .unwrap()
                .as_str(),
            "https://gateway.example/api"
        );
        assert!(
            GatewayApiUrl::try_from(
                "iroh://8a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5c"
            )
            .is_ok()
        );
        for invalid in [
            "http://gateway.example",
            "https://127.0.0.1",
            "https://gateway",
            "https://user@gateway.example",
            "https://gateway.example/api?token=secret",
            "https://gateway.example/api#fragment",
            "file:///tmp/gateway",
            "iroh://not-a-node-id",
        ] {
            assert!(GatewayApiUrl::try_from(invalid).is_err(), "{invalid}");
        }
    }
}
