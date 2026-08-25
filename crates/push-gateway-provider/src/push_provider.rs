use std::{future::Future, pin::Pin};

use fedi_decentralized_push_gateway_types::{FcmRegistrationToken, Notification, PushRegistration};

/// Boxed future returned by push provider implementations.
pub type ProviderFuture<'a> =
    Pin<Box<dyn Future<Output = Result<(), PushProviderError>> + Send + 'a>>;

/// Provider abstraction for registration validation and notification delivery.
pub trait PushProvider: Send + Sync {
    /// Validates that a registration token belongs to the configured provider
    /// project without delivering a user-visible notification.
    ///
    /// Implementations may perform network I/O and provider authentication. The
    /// call is admission-critical: only a definitive token/provider rejection
    /// may return [`PushProviderErrorKind::InvalidToken`], which the HTTP layer
    /// exposes as 422. Invalid validation payloads, auth/quota failures, timeouts,
    /// network errors, and ambiguous provider responses must return
    /// [`PushProviderErrorKind::InvalidPayload`] or
    /// [`PushProviderErrorKind::Unavailable`]. Those classes fail closed as 503,
    /// and the caller must not persist or alter registration ownership.
    fn validate_registration<'a>(&'a self, token: &'a FcmRegistrationToken) -> ProviderFuture<'a>;

    /// Delivers one notification to one registered app installation.
    fn deliver<'a>(
        &'a self,
        registration: &'a PushRegistration,
        notification: &'a Notification,
    ) -> ProviderFuture<'a>;
}

/// Sanitized provider-operation error shared by validation and delivery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PushProviderError {
    /// Static sanitized reason code.
    pub reason: &'static str,
    /// Provider-operation error classification.
    kind: PushProviderErrorKind,
}

impl PushProviderError {
    /// Creates an unavailable-provider error with a sanitized reason code.
    #[must_use]
    pub fn unavailable(reason: &'static str) -> Self {
        Self {
            reason,
            kind: PushProviderErrorKind::Unavailable,
        }
    }

    /// Creates an invalid-token provider error.
    #[must_use]
    pub fn invalid_token(reason: &'static str) -> Self {
        Self {
            reason,
            kind: PushProviderErrorKind::InvalidToken,
        }
    }

    /// Creates an invalid-payload provider error.
    #[must_use]
    pub fn invalid_payload(reason: &'static str) -> Self {
        Self {
            reason,
            kind: PushProviderErrorKind::InvalidPayload,
        }
    }

    /// Returns true if this error means the registration token should be disabled.
    #[must_use]
    pub fn disables_registration(&self) -> bool {
        self.kind == PushProviderErrorKind::InvalidToken
    }

    /// Returns the sanitized provider-operation error classification.
    #[must_use]
    pub fn kind(&self) -> PushProviderErrorKind {
        self.kind
    }
}

/// Sanitized provider error class used by registration validation and delivery.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PushProviderErrorKind {
    /// The target token is permanently invalid or unregistered.
    InvalidToken,
    /// The provider request payload is permanently invalid, but the token is not.
    InvalidPayload,
    /// The provider, quota, or network operation is currently unavailable.
    Unavailable,
}

/// No-op provider used by default for local tests and `defe`.
#[derive(Clone, Debug, Default)]
pub struct NoopPushProvider;

impl PushProvider for NoopPushProvider {
    fn validate_registration<'a>(&'a self, _token: &'a FcmRegistrationToken) -> ProviderFuture<'a> {
        Box::pin(async { Ok(()) })
    }

    fn deliver<'a>(
        &'a self,
        _registration: &'a PushRegistration,
        _notification: &'a Notification,
    ) -> ProviderFuture<'a> {
        Box::pin(async { Ok(()) })
    }
}

/// One attempted fake push delivery captured in tests.
#[derive(Clone, Debug, PartialEq)]
pub struct FakeDelivery {
    /// Target push registration.
    pub registration: PushRegistration,
    /// Notification delivered to the target.
    pub notification: Notification,
}

/// Fake provider that records delivery attempts without network access.
#[derive(Clone, Debug, Default)]
pub struct FakePushProvider {
    deliveries: std::sync::Arc<std::sync::Mutex<Vec<FakeDelivery>>>,
}

impl FakePushProvider {
    /// Returns all recorded fake deliveries.
    #[must_use]
    pub fn deliveries(&self) -> Vec<FakeDelivery> {
        self.deliveries.lock().expect("fake delivery mutex").clone()
    }
}

impl PushProvider for FakePushProvider {
    fn validate_registration<'a>(&'a self, _token: &'a FcmRegistrationToken) -> ProviderFuture<'a> {
        Box::pin(async { Ok(()) })
    }

    fn deliver<'a>(
        &'a self,
        registration: &'a PushRegistration,
        notification: &'a Notification,
    ) -> ProviderFuture<'a> {
        Box::pin(async move {
            self.deliveries
                .lock()
                .expect("fake delivery mutex")
                .push(FakeDelivery {
                    registration: registration.clone(),
                    notification: notification.clone(),
                });
            Ok(())
        })
    }
}
