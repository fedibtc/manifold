//! Push delivery provider abstraction and lightweight implementations.

mod push_provider;

pub use push_provider::{
    FakeDelivery, FakePushProvider, NoopPushProvider, ProviderFuture, PushProvider,
    PushProviderError, PushProviderErrorKind,
};
