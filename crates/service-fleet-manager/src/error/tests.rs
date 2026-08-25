use crate::{FleetManagerError, Plan, ServiceStatus};

#[test]
fn thiserror_display_matches_wire_phrases() {
    assert_eq!(
        FleetManagerError::InvalidPayment.to_string(),
        "invalid payment"
    );
    assert_eq!(
        FleetManagerError::Other("boom".to_string()).to_string(),
        "no can do for boom"
    );
    assert_eq!(
        FleetManagerError::FederationIsRunning.to_string(),
        "federation is running"
    );
    assert_eq!(
        FleetManagerError::MetaTargetConflict.to_string(),
        "meta target conflict, base pinned to a different admitted value"
    );
}

#[test]
fn strum_display_uses_variant_names() {
    assert_eq!(
        Plan::InfiniteBestEffort { price_msats: 100 }.to_string(),
        "InfiniteBestEffort"
    );
}

#[test]
fn service_status_display_uses_wire_strings() {
    assert_eq!(ServiceStatus::New.to_string(), "new");
    assert_eq!(ServiceStatus::DkgInProcess.to_string(), "DKG in process");
    assert_eq!(ServiceStatus::DataLoss.to_string(), "guardian data loss");
    assert_eq!(ServiceStatus::Running.to_string(), "running");
}

#[test]
fn service_status_preserves_established_wire_names() {
    assert_eq!(
        serde_json::to_string(&ServiceStatus::New).unwrap(),
        r#""New""#
    );
    assert_eq!(
        serde_json::to_string(&ServiceStatus::DkgInProcess).unwrap(),
        r#""DkgInProcess""#
    );
    assert_eq!(
        serde_json::to_string(&ServiceStatus::Running).unwrap(),
        r#""Running""#
    );
}
