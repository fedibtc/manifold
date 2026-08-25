use serde_json::{Map, Value};

use fedi_decentralized_push_gateway_types::Notification;

/// Maximum accepted FCM data payload size after gateway-added keys.
pub const MAX_FCM_DATA_BYTES: usize = 4096;

/// Transport-agnostic FCM outbound payload validation error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FcmOutboundValidationError {
    /// Payload uses a key reserved by FCM.
    ReservedDataKey,
    /// Payload exceeds FCM's accepted data payload budget.
    DataTooLarge,
}

impl std::fmt::Display for FcmOutboundValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::ReservedDataKey => "data payload uses a reserved key",
            Self::DataTooLarge => "data payload is too large",
        })
    }
}

impl std::error::Error for FcmOutboundValidationError {}

/// Validates FCM-specific data-map constraints after all gateway keys are added.
pub fn validate_fcm_outbound_data(
    data: &Map<String, Value>,
) -> Result<(), FcmOutboundValidationError> {
    if data.keys().any(|key| is_fcm_reserved_data_key(key)) {
        return Err(FcmOutboundValidationError::ReservedDataKey);
    }
    if fcm_data_size(data) > MAX_FCM_DATA_BYTES {
        return Err(FcmOutboundValidationError::DataTooLarge);
    }
    Ok(())
}

/// Builds the final FCM data map, including gateway-added keys.
#[must_use]
pub fn fcm_outbound_data(notification: &Notification) -> Map<String, Value> {
    let mut data = notification.data.clone();
    data.insert(
        "recipient_id".to_owned(),
        Value::String(notification.recipient_id.0.clone()),
    );
    data.insert(
        "notification_id".to_owned(),
        Value::String(notification.notification_id.0.clone()),
    );
    data.insert(
        "kind".to_owned(),
        Value::String(notification.kind.0.clone()),
    );
    data
}

/// Validates the final FCM data map, including gateway-added keys.
pub fn validate_fcm_outbound_notification(
    notification: &Notification,
) -> Result<(), FcmOutboundValidationError> {
    validate_fcm_outbound_data(&fcm_outbound_data(notification))
}

/// Returns true when a data key is reserved by FCM and cannot be sent in
/// application-defined data payloads.
#[must_use]
pub fn is_fcm_reserved_data_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key == "from" || key == "message_type" || key.starts_with("google") || key.starts_with("gcm.")
}

fn fcm_data_size(data: &Map<String, Value>) -> usize {
    data.iter()
        .map(|(key, value)| key.len() + fcm_data_value(value).len())
        .sum()
}

pub(crate) fn fcm_data_value(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        value => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, Value};

    use super::{
        FcmOutboundValidationError, MAX_FCM_DATA_BYTES, fcm_data_value, validate_fcm_outbound_data,
    };

    #[test]
    fn fcm_outbound_data_size_counts_stringified_values() {
        let mut data = Map::new();
        data.insert("bool".to_owned(), Value::Bool(true));
        data.insert(
            "fill".to_owned(),
            Value::String("x".repeat(MAX_FCM_DATA_BYTES)),
        );
        assert_eq!(fcm_data_value(&Value::Bool(true)), "true");

        let err = validate_fcm_outbound_data(&data).expect_err("oversized data rejected");
        assert_eq!(err, FcmOutboundValidationError::DataTooLarge);
    }
}
