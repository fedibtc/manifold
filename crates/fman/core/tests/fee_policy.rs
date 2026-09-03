//! The recipient value is written by a different codebase, so these pin what
//! this FMan concludes about its own share from each shape that codebase can
//! produce — including the shapes it refuses, which must never look like a
//! share we hold.

use bitcoin::secp256k1::{PublicKey, SECP256K1, SecretKey};
use stability_pool_client::common::{Account, AccountType};

use super::*;

fn account(byte: u8) -> Account {
    let key = PublicKey::from_secret_key(
        SECP256K1,
        &SecretKey::from_slice(&[byte; 32]).expect("fixed test scalar is valid"),
    );
    Account::single(key, AccountType::BtcDepositor)
}

fn id(account: &Account) -> String {
    account.id().to_string()
}

fn list(entries: &[(&Account, u64)]) -> String {
    let recipients: Vec<_> = entries
        .iter()
        .map(|(account, weight)| {
            serde_json::json!({
                "account": account,
                "account_id": id(account),
                "weight": weight,
            })
        })
        .collect();
    serde_json::json!({ "version": 1, "recipients": recipients }).to_string()
}

#[test]
fn reads_our_weight_out_of_the_list() {
    let (ours, peer) = (account(0x11), account(0x22));
    let value = list(&[(&ours, 3), (&peer, 1)]);

    assert_eq!(our_share_of(&value, ours.id()), Some((3, 4)));
    assert_eq!(our_share_of(&value, peer.id()), Some((1, 4)));
}

#[test]
fn a_list_without_us_is_no_share() {
    let value = list(&[(&account(0x22), 1)]);
    assert_eq!(our_share_of(&value, account(0x11).id()), None);
}

/// One bare account with no weights holds the whole share.
#[test]
fn the_single_account_form_reads_as_one_whole_share() {
    let ours = account(0x11);
    let value = serde_json::to_string(&ours).expect("serialize account");

    assert_eq!(our_share_of(&value, ours.id()), Some((1, 1)));
    assert_eq!(our_share_of(&value, account(0x22).id()), None);
}

/// Values the payer refuses pay nobody, so concluding a share from one would
/// tell the operator it is being paid when it is not.
#[test]
fn values_the_payer_refuses_are_not_a_share() {
    let ours = account(0x11);
    let our_id = id(&ours);

    let future_version = serde_json::json!({
        "version": 2,
        "recipients": [{ "account": ours, "account_id": our_id, "weight": 1 }],
    })
    .to_string();
    assert_eq!(our_share_of(&future_version, ours.id()), None);

    let zero_weight = list(&[(&ours, 0)]);
    assert_eq!(our_share_of(&zero_weight, ours.id()), None);

    let empty = serde_json::json!({ "version": 1, "recipients": [] }).to_string();
    assert_eq!(our_share_of(&empty, ours.id()), None);

    assert_eq!(our_share_of("not json at all", ours.id()), None);
}

#[test]
fn validation_matches_the_payers_complete_policy() {
    let (ours, peer) = (account(0x11), account(0x22));
    let our_id = id(&ours);

    assert_eq!(
        validate_fee_policy(Some(MAX_SEND_PPM + 1), Some(&list(&[(&ours, 1)])))
            .unwrap_err()
            .to_string(),
        format!("guardian-fee rate exceeds the payer cap of {MAX_SEND_PPM} ppm")
    );
    assert!(matches!(
        validate_fee_policy(Some(1), None),
        Err(FeePolicyError::Incomplete)
    ));
    assert!(matches!(
        validate_fee_policy(None, Some(&list(&[(&ours, 1)]))),
        Err(FeePolicyError::Incomplete)
    ));
    assert!(matches!(
        validate_fee_policy(
            Some(1),
            Some(
                &serde_json::json!({
                    "version": 2,
                    "recipients": [{ "account_id": our_id, "weight": 1 }],
                })
                .to_string()
            )
        ),
        Err(FeePolicyError::InvalidRecipients)
    ));
    // A zero weight anywhere invalidates the whole list, not only our entry.
    assert!(matches!(
        validate_fee_policy(Some(1), Some(&list(&[(&ours, 1), (&peer, 0)]))),
        Err(FeePolicyError::InvalidRecipients)
    ));
    assert!(matches!(
        validate_fee_policy(Some(1), Some(r#"{"version":1,"recipients":[]}"#)),
        Err(FeePolicyError::InvalidRecipients)
    ));
    let accounts: Vec<Account> = (1..=MAX_RECIPIENTS as u8 + 1).map(account).collect();
    let entries: Vec<(&Account, u64)> = accounts.iter().map(|account| (account, 1)).collect();
    assert!(matches!(
        validate_fee_policy(Some(1), Some(&list(&entries))),
        Err(FeePolicyError::InvalidRecipients)
    ));
    assert!(matches!(
        validate_fee_policy(Some(1), Some(&list(&[(&ours, u64::MAX), (&peer, 2)]))),
        Err(FeePolicyError::InvalidRecipients)
    ));
}

#[test]
fn policy_reader_derives_a_view_from_the_consensus_meta_map() {
    let (ours, peer) = (account(0x11), account(0x22));
    let recipients = list(&[(&ours, 3), (&peer, 1)]);
    let meta = std::collections::BTreeMap::from([
        (SEND_PPM_META_KEY.to_owned(), "1000".to_owned()),
        (REMITTANCE_ACCOUNT_META_KEY.to_owned(), recipients.clone()),
    ]);

    assert_eq!(
        fee_policy_from_meta(&meta, ours.id()),
        FeePolicy {
            configured: true,
            send_ppm: Some(1_000),
            recipients: Some(recipients),
            our_share: Some((3, 4)),
            authenticated_policy_matches: false,
        }
    );
}

/// A list longer than the payer honours is refused there, so this end must not
/// report a share the payer will never send.
#[test]
fn an_over_long_list_is_not_a_share() {
    let accounts: Vec<Account> = (1..=MAX_RECIPIENTS as u8 + 1).map(account).collect();
    let entries: Vec<(&Account, u64)> = accounts.iter().map(|account| (account, 1)).collect();
    assert!(entries.len() > MAX_RECIPIENTS);

    assert_eq!(our_share_of(&list(&entries), accounts[0].id()), None);
}

/// Weights that would overflow the total must not wrap into a plausible share.
#[test]
fn overflowing_weights_are_not_a_share() {
    let (ours, peer) = (account(0x11), account(0x22));
    let value = list(&[(&ours, u64::MAX), (&peer, 2)]);

    assert_eq!(our_share_of(&value, ours.id()), None);
}

#[test]
fn share_policy_reports_the_complete_authenticated_policy_check() {
    let unset = FeePolicy {
        configured: false,
        send_ppm: None,
        recipients: None,
        our_share: None,
        authenticated_policy_matches: true,
    };
    assert!(unset.share_matches_policy());

    let expected = FeePolicy {
        configured: true,
        send_ppm: Some(1_000),
        recipients: Some(String::new()),
        our_share: Some((GUARDIAN_RECIPIENT_WEIGHT, 7)),
        authenticated_policy_matches: true,
    };
    assert!(expected.share_matches_policy());

    assert!(
        !FeePolicy {
            authenticated_policy_matches: false,
            ..expected.clone()
        }
        .share_matches_policy()
    );
    assert!(
        !FeePolicy {
            our_share: None,
            authenticated_policy_matches: false,
            ..expected
        }
        .share_matches_policy()
    );
}

/// The proposal policy is the only thing standing between an FI's proposal
/// and this FMan's vote, so each way it can refuse is pinned here rather than
/// only through the seat that calls it.
#[test]
fn a_proposal_must_pay_the_complete_authenticated_split() {
    let (ours, peer, fi, guardian_verification_fee_account) =
        (account(0x11), account(0x22), account(0x30), account(0x31));
    let proposal = |entries: &[(&Account, u64)]| {
        let mut recipients = entries
            .iter()
            .map(|(account, weight)| {
                GuardianFeeRecipient::new((*account).clone().try_into().unwrap(), *weight)
            })
            .collect::<Vec<_>>();
        recipients.sort_by_key(|recipient| recipient.account.as_account().id());
        recipients
    };

    let fixed = proposal(&[
        (&ours, GUARDIAN_RECIPIENT_WEIGHT),
        (&peer, GUARDIAN_RECIPIENT_WEIGHT),
        (&fi, FI_RECIPIENT_WEIGHT),
        (
            &guardian_verification_fee_account,
            GUARDIAN_VERIFICATION_FEE_WEIGHT,
        ),
    ]);
    let guardians = vec![ours.clone(), peer.clone()];
    assert!(canonical_proposal(1, &fixed, &guardians, &guardian_verification_fee_account).is_ok());

    assert!(matches!(
        canonical_proposal(
            1,
            &fixed,
            &[ours.clone(), peer.clone(), account(0x23)],
            &guardian_verification_fee_account
        ),
        Err(FeePolicyError::InvalidSplit {
            expected: 5,
            got: 4
        })
    ));
    assert!(matches!(
        canonical_proposal(
            1,
            &proposal(&[
                (&ours, 1),
                (&peer, 1),
                (&fi, 5),
                (&guardian_verification_fee_account, 1)
            ]),
            &guardians,
            &guardian_verification_fee_account,
        ),
        Err(FeePolicyError::InvalidSplit { .. })
    ));

    assert!(matches!(
        canonical_proposal(
            1,
            &proposal(&[
                (&peer, 1),
                (&account(0x23), 1),
                (&fi, 4),
                (&guardian_verification_fee_account, 1)
            ]),
            &guardians,
            &guardian_verification_fee_account,
        ),
        Err(FeePolicyError::InvalidSplit { .. })
    ));

    // FI and FMan identities are disjoint by construction, so a combined
    // weight-five FI-guardian entry is refused like any other wrong weight.
    let fi_guardian = proposal(&[
        (&ours, FI_RECIPIENT_WEIGHT + GUARDIAN_RECIPIENT_WEIGHT),
        (&peer, GUARDIAN_RECIPIENT_WEIGHT),
        (
            &guardian_verification_fee_account,
            GUARDIAN_VERIFICATION_FEE_WEIGHT,
        ),
    ]);
    assert!(matches!(
        canonical_proposal(
            1,
            &fi_guardian,
            &guardians,
            &guardian_verification_fee_account
        ),
        Err(FeePolicyError::InvalidSplit { .. })
    ));

    let mut duplicate = fixed.clone();
    duplicate[1] = duplicate[0].clone();
    assert!(matches!(
        canonical_proposal(
            1,
            &duplicate,
            &guardians,
            &guardian_verification_fee_account
        ),
        Err(FeePolicyError::InvalidRecipients)
    ));

    assert!(matches!(
        canonical_proposal(
            MAX_SEND_PPM + 1,
            &fixed,
            &guardians,
            &guardian_verification_fee_account
        ),
        Err(FeePolicyError::SendPpmTooHigh)
    ));
    assert!(
        canonical_proposal(
            MAX_SEND_PPM,
            &fixed,
            &guardians,
            &guardian_verification_fee_account
        )
        .is_ok()
    );
}

/// The published floor gates a *new* proposal and only a new proposal. These
/// pin both halves, because the second half is what keeps an existing
/// federation maintainable after the published minimum rises.
#[test]
fn the_published_minimum_gates_new_proposals_only() {
    let (ours, peer, fi, guardian_verification_fee_account) =
        (account(0x11), account(0x22), account(0x30), account(0x31));
    let mut recipients = [
        (&ours, GUARDIAN_RECIPIENT_WEIGHT),
        (&peer, GUARDIAN_RECIPIENT_WEIGHT),
        (&fi, FI_RECIPIENT_WEIGHT),
        (
            &guardian_verification_fee_account,
            GUARDIAN_VERIFICATION_FEE_WEIGHT,
        ),
    ]
    .iter()
    .map(|(account, weight)| {
        GuardianFeeRecipient::new((*account).clone().try_into().unwrap(), *weight)
    })
    .collect::<Vec<_>>();
    recipients.sort_by_key(|recipient| recipient.account.as_account().id());
    let guardians = vec![ours.clone(), peer.clone()];
    let minimum = fedi_decentralized_domain::DEFAULT_SETUP_PAYMENT_MIN_FEE_PPM;

    // At the floor, and anywhere above it, is a proposal this FMan will vote for.
    assert!(prevalidate_guardian_fee_proposal(minimum, Some(minimum), &recipients).is_ok());
    assert!(prevalidate_guardian_fee_proposal(minimum + 1, Some(minimum), &recipients).is_ok());
    assert!(prevalidate_guardian_fee_proposal(MAX_SEND_PPM, Some(minimum), &recipients).is_ok());

    // Below it — including zero — a proposal is refused.
    for send_ppm in [0, 1, minimum - 1] {
        assert!(matches!(
            prevalidate_guardian_fee_proposal(send_ppm, Some(minimum), &recipients),
            Err(FeePolicyError::SendPpmTooLow { minimum: reported }) if reported == minimum
        ));
    }

    // Both bounds still apply together: the ceiling is checked first.
    assert!(matches!(
        prevalidate_guardian_fee_proposal(MAX_SEND_PPM + 1, Some(minimum), &recipients),
        Err(FeePolicyError::SendPpmTooHigh)
    ));

    // The carry-forward and read paths pass no floor, so a rate agreed before
    // the floor was raised stays a valid canonical value. Without this, an
    // unrelated meta write on such a federation could not be voted for at all.
    for send_ppm in [0, 1, minimum - 1] {
        assert!(prevalidate_guardian_fee_proposal(send_ppm, None, &recipients).is_ok());
        let value = canonical_proposal(
            send_ppm,
            &recipients,
            &guardians,
            &guardian_verification_fee_account,
        )
        .expect("a sub-minimum rate is still a canonical fee policy");
        validate_canonical_proposal_value(
            send_ppm,
            &value,
            &guardians,
            &guardian_verification_fee_account,
        )
        .expect("revalidating a carried sub-minimum policy must not apply the floor");
        assert!(
            fee_policy_from_meta(
                &std::collections::BTreeMap::from([
                    (SEND_PPM_META_KEY.to_owned(), send_ppm.to_string()),
                    (REMITTANCE_ACCOUNT_META_KEY.to_owned(), value),
                ]),
                ours.id(),
            )
            .configured
        );
    }
}
