use super::*;
use crate::federation_stats::combine_stats;

const UTC_DAY: u64 = 20_000;

fn figure(value: u64, complete: bool) -> WindowedFigure {
    WindowedFigure { value, complete }
}

/// An answer whose windowed figures all report `windowed`.
fn answer(custodied_msats: u64, windowed: WindowedFigure) -> FedimintStats {
    FedimintStats {
        custodied_msats,
        active_wallets_30d: windowed,
        active_wallets_history: None,
        volume_1008_blocks_msats: windowed,
        volume_4320_blocks_msats: windowed,
    }
}

fn combined(value: u64, based_on: u16, partial: bool) -> CombinedFigure {
    CombinedFigure {
        value,
        based_on,
        partial,
    }
}

#[test]
fn the_median_of_complete_answers_ignores_partial_ones() {
    // Four guardians, so 2f+1 is three. The median of every answer is 10.
    let stats = combine_stats(
        4,
        &[
            answer(1, figure(90, true)),
            answer(1, figure(5, false)),
            answer(1, figure(11, true)),
            answer(1, figure(10, true)),
        ],
        UTC_DAY,
    )
    .expect("answers");
    assert_eq!(stats.volume_1008_blocks_msats, combined(11, 3, false));
}

#[test]
fn exactly_2f_plus_one_complete_answers_take_the_lower_median() {
    let stats = combine_stats(
        4,
        &[
            answer(1, figure(60, true)),
            answer(1, figure(1, false)),
            answer(1, figure(50, true)),
            answer(1, figure(40, true)),
        ],
        UTC_DAY,
    )
    .expect("answers");
    assert_eq!(stats.active_wallets_30d, combined(50, 3, false));
}

#[test]
fn f_plus_one_complete_answers_are_still_partial() {
    // Two complete answers out of four would let one dishonest guardian pick
    // the value, so the figure is the partial median of every answer.
    let stats = combine_stats(
        4,
        &[
            answer(1, figure(100, true)),
            answer(1, figure(90, true)),
            answer(1, figure(8, false)),
        ],
        UTC_DAY,
    )
    .expect("answers");
    assert_eq!(stats.volume_4320_blocks_msats, combined(90, 3, true));
}

#[test]
fn one_dishonest_custody_answer_cannot_set_the_value() {
    let two = combine_stats(
        4,
        &[answer(700, figure(0, false)), answer(0, figure(0, false))],
        UTC_DAY,
    )
    .expect("answers");
    assert_eq!(two.custodied_msats, combined(0, 2, true));
    // With three answers, the dishonest 0 sits below both honest ones.
    let three = combine_stats(
        4,
        &[
            answer(700, figure(0, false)),
            answer(0, figure(0, false)),
            answer(500, figure(0, false)),
        ],
        UTC_DAY,
    )
    .expect("answers");
    assert_eq!(three.custodied_msats, combined(500, 3, false));
    assert_eq!(three.guardians, 4);
}

#[test]
fn larger_federations_need_more_complete_answers() {
    // Seven guardians tolerate two faulty ones, so 2f+1 is five.
    let four = [40, 30, 20, 10].map(|value| answer(1, figure(value, true)));
    let stats = combine_stats(7, &four, UTC_DAY).expect("answers");
    assert_eq!(stats.volume_1008_blocks_msats, combined(20, 4, true));
    let five = [40, 30, 20, 10, 50].map(|value| answer(1, figure(value, true)));
    let stats = combine_stats(7, &five, UTC_DAY).expect("answers");
    assert_eq!(stats.volume_1008_blocks_msats, combined(30, 5, false));
}

#[test]
fn no_answers_give_no_stats() {
    assert_eq!(combine_stats(4, &[], UTC_DAY), None);
}

fn dated_answer(utc_day: u64, current: u64, previous: u64) -> FedimintStats {
    FedimintStats {
        active_wallets_history: Some(ActiveWalletHistory {
            utc_day,
            previous_30d: Some(figure(previous, true)),
        }),
        ..answer(1, figure(current, true))
    }
}

#[test]
fn comparison_uses_both_medians_from_the_same_guardians_not_a_median_of_deltas() {
    let stats = combine_stats(
        4,
        &[
            dated_answer(UTC_DAY, 9, 42),
            dated_answer(UTC_DAY, 20, 8),
            dated_answer(UTC_DAY, 50, 24),
            // A complete legacy answer changes the standalone median from 20
            // to 9 but must not change the comparison.
            answer(1, figure(1, true)),
        ],
        UTC_DAY,
    )
    .expect("answers");

    assert_eq!(stats.active_wallets_30d, combined(9, 4, false));
    let comparison = stats
        .active_wallets_comparison
        .expect("three complete pairs");
    assert_eq!(
        comparison,
        ActiveWalletComparison {
            utc_day: UTC_DAY,
            current_30d: 20,
            previous_30d: 24,
            based_on: 3,
        }
    );
    assert_eq!(comparison.delta(), -4);
    assert!((comparison.delta_percent().unwrap() - (-16.666666666666668)).abs() < 1e-12);
}

#[test]
fn comparison_requires_a_quorum_of_complete_pairs_on_the_intended_day() {
    let valid = dated_answer(UTC_DAY, 12, 5);
    let history = valid.active_wallets_history.unwrap();
    let without_baseline = FedimintStats {
        active_wallets_history: Some(ActiveWalletHistory {
            previous_30d: None,
            ..history
        }),
        ..valid.clone()
    };
    let with_partial_baseline = FedimintStats {
        active_wallets_history: Some(ActiveWalletHistory {
            previous_30d: Some(figure(5, false)),
            ..history
        }),
        ..valid.clone()
    };
    let with_partial_current = FedimintStats {
        active_wallets_30d: figure(12, false),
        ..valid.clone()
    };
    for invalid in [
        without_baseline,
        with_partial_baseline,
        with_partial_current,
        dated_answer(UTC_DAY - 1, 12, 5),
        dated_answer(UTC_DAY + 1, 12, 5),
    ] {
        let stats = combine_stats(
            4,
            &[valid.clone(), dated_answer(UTC_DAY, 2, 1), invalid],
            UTC_DAY,
        )
        .expect("answers");
        assert_eq!(stats.active_wallets_comparison, None);
    }

    let yesterday = dated_answer(UTC_DAY - 1, 12, 5);
    let stats = combine_stats(
        4,
        &[yesterday.clone(), yesterday.clone(), yesterday, valid],
        UTC_DAY,
    )
    .expect("answers");
    assert_eq!(stats.active_wallets_comparison, None, "no stale fallback");
}

#[test]
fn zero_baseline_is_available_without_a_percentage() {
    let stats = combine_stats(1, &[dated_answer(UTC_DAY, u64::MAX, 0)], UTC_DAY).expect("answers");
    let comparison = stats.active_wallets_comparison.expect("zero is a count");
    assert_eq!(comparison.previous_30d, 0);
    assert_eq!(comparison.delta(), i128::from(u64::MAX));
    assert_eq!(comparison.delta_percent(), None);
}

#[tokio::test]
async fn federation_stats_combine_the_guardians_that_answer() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let reader = TestConsensusReader::new(mg_state.clone());
    let client = open_client_with_reader(
        database,
        payments,
        mg_state.clone(),
        MgConfig::given_away(),
        reader,
    )
    .await;
    client
        .create_at(intent(), locators(), options())
        .await
        .unwrap();

    // Seven guardians, so 2f+1 is five. Guardian 0 is offline, guardian 6
    // fails, and guardians 1 to 5 answer.
    assert_eq!(locators().len(), 7);
    *mg_state.offline_indices.lock().expect("test lock") = [0].into_iter().collect();
    *mg_state.fedimint_stats.lock().expect("test lock") = [
        (0, answer(1, figure(1, true))),
        (1, answer(300, figure(20, true))),
        (2, answer(200, figure(10, true))),
        (3, answer(900, figure(12, true))),
        (4, answer(400, figure(15, true))),
        (5, answer(100, figure(11, true))),
    ]
    .into_iter()
    .collect();
    let stats = client.federation_stats().await.unwrap();
    assert_eq!(
        stats,
        FederationStats {
            guardians: 7,
            custodied_msats: combined(300, 5, false),
            active_wallets_30d: combined(12, 5, false),
            active_wallets_comparison: None,
            volume_1008_blocks_msats: combined(12, 5, false),
            volume_4320_blocks_msats: combined(12, 5, false),
        }
    );

    let now = test_now_secs();
    let today = now / 86_400;
    *mg_state.fedimint_stats.lock().expect("test lock") = [
        (1, dated_answer(today, 12, 32)),
        (2, dated_answer(today, 25, 10)),
        (3, dated_answer(today, 17, 12)),
        (4, dated_answer(today, 50, 30)),
        (5, dated_answer(today, 2, 42)),
        // An extra complete pair for another date affects only the total.
        (6, dated_answer(today - 1, 5, 1)),
    ]
    .into_iter()
    .collect();
    let stats = crate::formation::TEST_NOW_SECS
        .scope(Arc::new(AtomicU64::new(now)), client.federation_stats())
        .await
        .unwrap();
    assert_eq!(stats.active_wallets_30d, combined(12, 6, false));
    assert_eq!(
        stats.active_wallets_comparison,
        Some(ActiveWalletComparison {
            utc_day: today,
            current_30d: 17,
            previous_30d: 30,
            based_on: 5,
        })
    );

    // With no answers, the error separates guardians without the verb from
    // guardians that failed now.
    mg_state.fedimint_stats.lock().expect("test lock").clear();
    // Guardians 1 to 5 run an MG from before stats, guardian 6 one from
    // before `GetVersion`.
    *mg_state.versions.lock().expect("test lock") = (1..6)
        .map(|index| (index, TestMgVersion::WithoutStats))
        .chain([(6, TestMgVersion::BeforeGetVersion)])
        .collect();
    let error = client.federation_stats().await.unwrap_err();
    assert!(
        matches!(
            &error,
            FaError::FederationStatsUnavailable { unsupported, failed }
                if unsupported == &[1, 2, 3, 4, 5, 6]
                    && failed.iter().map(|(index, _)| *index).collect::<Vec<_>>() == [0]
        ),
        "{error:?}"
    );
    mg_state.offline_indices.lock().expect("test lock").clear();
    let error = client.federation_stats().await.unwrap_err();
    assert!(
        matches!(
            &error,
            FaError::FederationStatsUnavailable { unsupported, failed }
                if unsupported.len() == 6 && failed.iter().map(|(index, _)| *index).eq([0])
        ),
        "guardian 0 is at the stats version but has none to report: {error:?}"
    );
    mg_state
        .versions
        .lock()
        .expect("test lock")
        .insert(0, TestMgVersion::WithoutStats);
    let error = client.federation_stats().await.unwrap_err();
    assert!(
        matches!(
            &error,
            FaError::FederationStatsUnavailable { unsupported, failed }
                if unsupported.len() == 7 && failed.is_empty()
        ),
        "{error:?}"
    );
}

#[tokio::test]
async fn federation_stats_need_a_formed_federation() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let client = open_client(database, payments, mg_state, MgConfig::given_away()).await;
    assert!(matches!(
        client.federation_stats().await,
        Err(FaError::NoActiveFormation)
    ));
}
