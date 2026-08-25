use serde_json::{Map, Value};

use super::validate_no_reserved_data_keys;

#[test]
fn fcm_reserved_keys_are_rejected_safely() {
    for key in ["from", "message_type", "google.foo", "googlex", "gcm.foo"] {
        let mut data = Map::new();
        data.insert(key.to_owned(), Value::String("value".to_owned()));
        let err = validate_no_reserved_data_keys(&data).expect_err("reserved key rejected");
        assert_eq!(err.code, "data_key_reserved");
        assert!(!err.message.contains(key));
    }
}
