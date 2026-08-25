use super::*;

#[test]
fn policy_mapping_distinguishes_malformed_invites_from_endpoint_rejections() {
    let error = map_endpoint_policy_error(EndpointPolicyError::MalformedInvite);
    assert!(matches!(
        error,
        FederationPreviewError::InvalidInviteCode(reason) if reason == "invite code is malformed"
    ));

    let error = map_endpoint_policy_error(EndpointPolicyError::UnsupportedScheme);
    assert!(matches!(
        error,
        FederationPreviewError::EndpointPolicyRejected
    ));
}
