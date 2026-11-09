use super::*;
use crate::federation_stats::combine_stats;

fn figure(value: u64, complete: bool) -> WindowedFigure {
    WindowedFigure { value, complete }
}

/// An answer whose windowed figures all report `windowed`.
fn answer(custodied_msats: u64, windowed: WindowedFigure) -> FedimintStats {
    FedimintStats {
        custodied_msats,
        active_wallets_30d: windowed,
        volume_7d_msats: windowed,
        volume_30d_msats: windowed,
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
    // Four guardians, so f+1 is two. The mean of the complete answers is 37
    // and the median of every answer is 10.
    let stats = combine_stats(
        4,
        &[
            answer(1, figure(90, true)),
            answer(1, figure(5, false)),
            answer(1, figure(11, true)),
            answer(1, figure(10, true)),
        ],
    )
    .expect("answers");
    assert_eq!(stats.volume_7d_msats, combined(11, 3, false));
}

#[test]
fn exactly_f_plus_one_complete_answers_take_the_lower_median() {
    let stats = combine_stats(
        4,
        &[
            answer(1, figure(60, true)),
            answer(1, figure(1, false)),
            answer(1, figure(50, true)),
        ],
    )
    .expect("answers");
    assert_eq!(stats.active_wallets_30d, combined(50, 2, false));
}

#[test]
fn up_to_f_complete_answers_give_a_partial_median_of_every_answer() {
    let stats = combine_stats(
        4,
        &[
            answer(1, figure(100, true)),
            answer(1, figure(7, false)),
            answer(1, figure(8, false)),
        ],
    )
    .expect("answers");
    assert_eq!(stats.volume_30d_msats, combined(8, 3, true));
}

#[test]
fn custody_counts_every_answer_and_needs_f_plus_one_of_them() {
    let one = combine_stats(4, &[answer(500, figure(0, false))]).expect("answer");
    assert_eq!(one.custodied_msats, combined(500, 1, true));
    let two = combine_stats(
        4,
        &[answer(700, figure(0, false)), answer(500, figure(0, false))],
    )
    .expect("answers");
    assert_eq!(two.custodied_msats, combined(500, 2, false));
    assert_eq!(two.guardians, 4);
}

#[test]
fn larger_federations_need_more_complete_answers() {
    // Seven guardians tolerate two faulty ones, so two complete answers are
    // not enough.
    let stats = combine_stats(
        7,
        &[
            answer(1, figure(40, true)),
            answer(1, figure(30, true)),
            answer(1, figure(2, false)),
        ],
    )
    .expect("answers");
    assert_eq!(stats.volume_7d_msats, combined(30, 3, true));
}

#[test]
fn no_answers_give_no_stats() {
    assert_eq!(combine_stats(4, &[]), None);
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

    // Seven guardians, so f+1 is three. Guardian 0 is offline, guardians 4
    // to 6 fail, and guardians 1 to 3 answer.
    assert!(MG_API_VERSION >= FEDIMINT_STATS_API_VERSION);
    assert_eq!(locators().len(), 7);
    *mg_state.offline_indices.lock().expect("test lock") = [0].into_iter().collect();
    *mg_state.fedimint_stats.lock().expect("test lock") = [
        (0, answer(1, figure(1, true))),
        (1, answer(300, figure(20, true))),
        (2, answer(200, figure(10, true))),
        (3, answer(900, figure(12, true))),
    ]
    .into_iter()
    .collect();
    let stats = client.federation_stats().await.unwrap();
    assert_eq!(
        stats,
        FederationStats {
            guardians: 7,
            custodied_msats: combined(300, 3, false),
            active_wallets_30d: combined(12, 3, false),
            volume_7d_msats: combined(12, 3, false),
            volume_30d_msats: combined(12, 3, false),
        }
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
