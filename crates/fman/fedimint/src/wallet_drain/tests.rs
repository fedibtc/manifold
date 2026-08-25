use super::*;

fn operation(state: OutgoingState, active: bool) -> OutgoingOperation {
    OutgoingOperation::new(
        PayoutOperationId::parse(&"01".repeat(32)).unwrap(),
        OutgoingRail::Lnv2,
        state,
        900,
        1_000,
        active,
    )
}

#[test]
fn constructor_and_state_transition_keep_encumbrance_coupled() {
    for (state, active, expected) in [
        (OutgoingState::Pending, false, None),
        (OutgoingState::Pending, true, None),
        (OutgoingState::Succeeded, false, Some(0)),
        (OutgoingState::Succeeded, true, Some(0)),
        (OutgoingState::FailedOrRefunded, false, Some(0)),
        (OutgoingState::FailedOrRefunded, true, Some(1_000)),
        (OutgoingState::Unknown, false, None),
        (OutgoingState::Unknown, true, None),
    ] {
        let constructed = operation(state, active);
        assert_eq!(constructed.state(), state);
        assert_eq!(constructed.contract_amount_msat(), 1_000);
        assert_eq!(constructed.has_active_state_machines(), active);
        assert_eq!(constructed.encumbered_msat(), expected);

        for source in [
            OutgoingState::Pending,
            OutgoingState::Succeeded,
            OutgoingState::FailedOrRefunded,
            OutgoingState::Unknown,
        ] {
            let transitioned = operation(source, active).with_state(state);
            assert_eq!(transitioned.state(), state);
            assert_eq!(transitioned.encumbered_msat(), expected);
            assert_eq!(transitioned.contract_amount_msat(), 1_000);
            assert_eq!(transitioned.has_active_state_machines(), active);
        }
    }
}

#[test]
fn active_change_after_v2_success_is_not_drained() {
    let status = WalletDrainStatus::new(
        Ok(Msats(0)),
        Ok(Msats(0)),
        Ok(vec![operation(OutgoingState::Succeeded, true)]),
        1,
    );

    assert_eq!(status.drain_state, DrainState::PendingWalletWork);
    assert_eq!(status.encumbered_outgoing_msat, Some(0));
}

#[test]
fn funding_rejection_with_mint_auto_refund_active_is_not_drained() {
    let status = WalletDrainStatus::new(
        Ok(Msats(0)),
        Ok(Msats(0)),
        Ok(vec![operation(OutgoingState::FailedOrRefunded, true)]),
        1,
    );

    assert_eq!(status.drain_state, DrainState::PendingWalletWork);
}

#[test]
fn cancellation_waiting_for_refund_output_is_not_drained() {
    let status = WalletDrainStatus::new(
        Ok(Msats(0)),
        Ok(Msats(0)),
        Ok(vec![operation(OutgoingState::Pending, true)]),
        1,
    );

    assert_eq!(status.drain_state, DrainState::PendingWalletWork);
}

#[test]
fn active_operation_without_cached_outcome_remains_pending_after_restart() {
    let status = WalletDrainStatus::new(
        Ok(Msats(0)),
        Ok(Msats(0)),
        Ok(vec![operation(OutgoingState::Pending, true)]),
        1,
    );

    assert_eq!(status.drain_state, DrainState::PendingWalletWork);
}

#[test]
fn inactive_operation_without_cached_outcome_is_unknown_after_restart() {
    let status = WalletDrainStatus::new(
        Ok(Msats(0)),
        Ok(Msats(0)),
        Ok(vec![operation(OutgoingState::Unknown, false)]),
        0,
    );

    assert_eq!(status.drain_state, DrainState::Unknown);
}

#[test]
fn every_query_error_fails_closed() {
    let status = WalletDrainStatus::new(
        Err(WalletDrainQuery::AvailableEcash),
        Ok(Msats(0)),
        Ok(Vec::new()),
        0,
    );

    assert_eq!(status.drain_state, DrainState::Unknown);
    assert_eq!(status.query_errors, vec![WalletDrainQuery::AvailableEcash]);
}

#[test]
fn fee_query_error_does_not_call_available_ecash_drained() {
    let status = WalletDrainStatus::new(
        Ok(Msats(100)),
        Err(WalletDrainQuery::EconomicallySweepable),
        Ok(Vec::new()),
        0,
    );

    assert_eq!(status.available_ecash_msat, Some(100));
    assert_eq!(status.drain_state, DrainState::Unknown);
}
