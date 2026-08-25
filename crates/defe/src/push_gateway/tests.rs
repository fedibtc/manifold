use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use super::*;

#[test]
fn managed_push_gateway_sets_local_public_base_url_escape_hatch() {
    let stable = StablePushGatewayAllocation {
        port: 32123,
        slot_dir: PathBuf::from("/tmp/slot"),
        database_path: PathBuf::from("/tmp/slot/push.sqlite"),
        app_id: "test-app".to_owned(),
    };

    let config = push_gateway_process_config(
        OsString::from("push-gateway"),
        Path::new("/tmp/push-gateway.log"),
        &stable,
    );
    let env = config
        .envs
        .iter()
        .map(|(key, value)| {
            (
                key.to_string_lossy().into_owned(),
                value.to_string_lossy().into_owned(),
            )
        })
        .collect::<std::collections::HashMap<_, _>>();

    assert_eq!(
        env.get("PUSH_GATEWAY_PUBLIC_BASE_URL").map(String::as_str),
        Some("http://127.0.0.1:32123")
    );
    assert_eq!(
        env.get("PUSH_GATEWAY_ALLOW_INSECURE_PUBLIC_BASE_URL")
            .map(String::as_str),
        Some("true")
    );
}
