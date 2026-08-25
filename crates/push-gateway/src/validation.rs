use serde_json::{Map, Value};

use fedi_decentralized_push_gateway_provider_fcm::is_fcm_reserved_data_key;

use crate::{ApiError, Notification};

/// Maximum accepted title length.
pub(crate) const MAX_TITLE_LEN: usize = 120;
/// Maximum accepted body length.
pub(crate) const MAX_BODY_LEN: usize = 512;
/// Maximum accepted label length.
pub(crate) const MAX_LABEL_LEN: usize = 120;
/// Maximum accepted serialized data payload length.
pub(crate) const MAX_DATA_JSON_LEN: usize = 4096;
/// Maximum accepted number of top-level data keys.
pub(crate) const MAX_DATA_KEYS: usize = 32;
/// Maximum accepted top-level data key length.
pub(crate) const MAX_DATA_KEY_LEN: usize = 64;
/// Maximum accepted opaque identifier length.
pub(crate) const MAX_ID_LEN: usize = 128;
/// Maximum accepted notification kind length.
pub(crate) const MAX_KIND_LEN: usize = 128;
/// Maximum accepted FCM registration token length.
pub(crate) const MAX_FCM_TOKEN_LEN: usize = 4096;
/// Maximum accepted platform label length.
pub(crate) const MAX_PLATFORM_LEN: usize = 32;

/// Validates an optional short string field.
pub(crate) fn validate_optional_string(
    name: &'static str,
    value: Option<&str>,
    max_len: usize,
) -> Result<(), ApiError> {
    if let Some(value) = value
        && value.len() > max_len
    {
        return Err(ApiError::bad_request(
            "field_too_long",
            field_too_long_message(name),
        ));
    }
    Ok(())
}

/// Validates a constrained JSON data map.
pub(crate) fn validate_data(data: &Map<String, Value>) -> Result<(), ApiError> {
    if data.len() > MAX_DATA_KEYS {
        return Err(ApiError::bad_request(
            "data_too_many_keys",
            "data payload has too many keys",
        ));
    }
    if data
        .keys()
        .any(|key| key.is_empty() || key.len() > MAX_DATA_KEY_LEN)
    {
        return Err(ApiError::bad_request(
            "data_key_invalid",
            "data payload has an invalid key",
        ));
    }
    if serde_json::to_string(data)
        .map(|json| json.len() > MAX_DATA_JSON_LEN)
        .unwrap_or(true)
    {
        return Err(ApiError::bad_request(
            "data_too_large",
            "data payload is too large",
        ));
    }
    Ok(())
}

/// Validates free-form notification data against gateway- and FCM-reserved key names.
pub(crate) fn validate_no_reserved_data_keys(data: &Map<String, Value>) -> Result<(), ApiError> {
    if data
        .keys()
        .any(|key| is_reserved_data_key(key) || is_fcm_reserved_data_key(key))
    {
        return Err(ApiError::bad_request(
            "data_key_reserved",
            "data payload uses a reserved key",
        ));
    }
    Ok(())
}

/// Validates the final FCM data map, including gateway-added keys.
pub(crate) fn validate_fcm_outbound_notification(
    notification: &Notification,
) -> Result<(), ApiError> {
    fedi_decentralized_push_gateway_provider_fcm::validate_fcm_outbound_notification(notification)
        .map_err(|err| match err {
            fedi_decentralized_push_gateway_provider_fcm::FcmOutboundValidationError::ReservedDataKey => {
                ApiError::bad_request("data_key_reserved", "data payload uses a reserved key")
            }
            fedi_decentralized_push_gateway_provider_fcm::FcmOutboundValidationError::DataTooLarge => {
                ApiError::bad_request("data_too_large", "data payload is too large")
            }
        })
}

/// Validates a required opaque identifier-like string.
pub(crate) fn validate_required_string(
    name: &'static str,
    value: &str,
    max_len: usize,
) -> Result<(), ApiError> {
    if value.trim().is_empty() {
        return Err(ApiError::bad_request(
            "field_required",
            field_required_message(name),
        ));
    }
    if value.len() > max_len {
        return Err(ApiError::bad_request(
            "field_too_long",
            field_too_long_message(name),
        ));
    }
    Ok(())
}

fn field_required_message(name: &'static str) -> &'static str {
    match name {
        "app_id" => "app_id is required",
        "recipient_id" => "recipient_id is required",
        "installation_id" => "installation_id is required",
        "fcm_token" => "fcm_token is required",
        "hook_id" => "hook_id is required",
        _ => "field is required",
    }
}

fn field_too_long_message(name: &'static str) -> &'static str {
    match name {
        "app_id" => "app_id is too long",
        "recipient_id" => "recipient_id is too long",
        "installation_id" => "installation_id is too long",
        "fcm_token" => "fcm_token is too long",
        "platform" => "platform is too long",
        "label" => "label is too long",
        "title" => "title is too long",
        "body" => "body is too long",
        "event_id" => "event_id is too long",
        "kind" => "kind is too long",
        _ => "field is too long",
    }
}

fn is_reserved_data_key(key: &str) -> bool {
    key.starts_with("pg.")
        || matches!(
            key,
            "recipient_id"
                | "notification_id"
                | "kind"
                | "workflow"
                | "action"
                | "deep_link"
                | "open_behavior"
                | "event_id"
        )
}

#[cfg(test)]
mod tests;
