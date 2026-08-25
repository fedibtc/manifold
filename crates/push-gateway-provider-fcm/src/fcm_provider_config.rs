use crate::FirebaseCredentials;

/// FCM HTTP v1 provider configuration.
#[derive(Clone, Eq, PartialEq)]
pub struct FcmProviderConfig {
    credentials: FirebaseCredentials,
    send_endpoint_base: String,
    max_concurrency: usize,
}

impl std::fmt::Debug for FcmProviderConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FcmProviderConfig")
            .field("credentials", &self.credentials)
            .field("send_endpoint_base", &self.send_endpoint_base)
            .field("max_concurrency", &self.max_concurrency)
            .finish()
    }
}

impl FcmProviderConfig {
    /// Creates FCM provider config from parsed service-account credentials.
    #[must_use]
    pub fn new(credentials: FirebaseCredentials) -> Self {
        Self {
            credentials,
            send_endpoint_base: "https://fcm.googleapis.com".to_owned(),
            max_concurrency: 16,
        }
    }

    /// Sets the FCM endpoint base URL, primarily for fake-server tests.
    #[must_use]
    pub fn with_send_endpoint_base(mut self, send_endpoint_base: impl Into<String>) -> Self {
        self.send_endpoint_base = send_endpoint_base.into();
        self
    }

    /// Sets the maximum number of concurrent provider HTTP requests.
    #[must_use]
    pub fn with_max_concurrency(mut self, max_concurrency: usize) -> Self {
        self.max_concurrency = max_concurrency.max(1);
        self
    }

    /// Returns parsed Firebase credentials.
    #[must_use]
    pub fn credentials(&self) -> &FirebaseCredentials {
        &self.credentials
    }

    /// Returns the FCM send endpoint base URL.
    #[must_use]
    pub fn send_endpoint_base(&self) -> &str {
        &self.send_endpoint_base
    }

    /// Returns the maximum provider HTTP concurrency.
    #[must_use]
    pub fn max_concurrency(&self) -> usize {
        self.max_concurrency
    }
}
