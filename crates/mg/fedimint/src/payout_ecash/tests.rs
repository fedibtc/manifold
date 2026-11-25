use super::*;

/// The selector ignores the requested amount in both directions: a payout is
/// the whole balance even when notes arrived after the balance was read.
#[tokio::test]
async fn whole_balance_selection_takes_every_note_whatever_is_requested() {
    let notes = || {
        futures::stream::iter(vec![
            (Amount::from_msats(1), 'a'),
            (Amount::from_msats(1), 'b'),
            (Amount::from_msats(4), 'c'),
            (Amount::from_msats(1_024), 'd'),
        ])
    };
    for requested in [1, 7, 1_030, 5_000] {
        let selected = AllNotes
            .select_notes(notes(), Amount::from_msats(requested), FeeConsensus::zero())
            .await
            .unwrap();
        assert_eq!(selected.count_items(), 4, "requested {requested}");
        assert_eq!(selected.total_amount(), Amount::from_msats(1_030));
    }
    assert!(
        AllNotes
            .select_notes(
                futures::stream::iter(Vec::<(Amount, char)>::new()),
                Amount::from_msats(1),
                FeeConsensus::zero(),
            )
            .await
            .is_err()
    );
}

/// A refused reclaim means the notes were already spent; only a terminal
/// reissue decides, so an in-flight one reads as no decision yet.
#[test]
fn reclaim_outcomes_map_to_payout_states() {
    assert_eq!(
        v1_reclaim_state(ReissueExternalNotesState::Done),
        Some(EcashPayoutState::Reclaimed)
    );
    assert_eq!(
        v1_reclaim_state(ReissueExternalNotesState::Failed("spent".into())),
        Some(EcashPayoutState::AlreadySpent)
    );
    assert_eq!(v1_reclaim_state(ReissueExternalNotesState::Created), None);
    assert_eq!(v1_reclaim_state(ReissueExternalNotesState::Issuing), None);
    assert_eq!(
        v2_reclaim_state(MintV2ReceiveState::Success),
        EcashPayoutState::Reclaimed
    );
    assert_eq!(
        v2_reclaim_state(MintV2ReceiveState::Rejected),
        EcashPayoutState::AlreadySpent
    );
}

#[test]
fn only_issued_and_reclaiming_payouts_are_open() {
    assert!(
        EcashPayoutState::Issued {
            token: EcashToken::test("t")
        }
        .is_open()
    );
    assert!(EcashPayoutState::Reclaiming.is_open());
    for closed in [EcashPayoutState::Reclaimed, EcashPayoutState::AlreadySpent] {
        assert!(!closed.is_open(), "{closed:?}");
    }
}

/// A token formatted into a log or error by accident must not leak.
#[test]
fn bearer_tokens_never_format() {
    let token = "fedimint1secretnotes";
    let state = EcashPayoutState::Issued {
        token: EcashToken::test(token),
    };
    assert!(!format!("{state:?}").contains(token));
    let wire = state.to_wire(true);
    assert!(!format!("{wire:?}").contains(token));
    assert_eq!(
        serde_json::to_value(&wire).unwrap(),
        serde_json::json!({"state": "issued", "token": token})
    );
    // The list's shape leaves it out.
    assert_eq!(
        serde_json::to_value(state.to_wire(false)).unwrap(),
        serde_json::json!({"state": "issued", "token": null})
    );
}
