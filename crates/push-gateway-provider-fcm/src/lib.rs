//! Firebase Cloud Messaging push provider implementation.

mod fcm_provider_config;
mod fcm_push_provider;
mod firebase_credentials;
mod outbound_validation;

pub use fcm_provider_config::FcmProviderConfig;
pub use fcm_push_provider::{FcmProviderError, FcmPushProvider};
pub use firebase_credentials::{FirebaseCredentials, FirebaseCredentialsError};
pub use outbound_validation::{
    FcmOutboundValidationError, fcm_outbound_data, is_fcm_reserved_data_key,
    validate_fcm_outbound_data, validate_fcm_outbound_notification,
};
