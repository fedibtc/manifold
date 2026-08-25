use super::{
    FederationId, FederationSize, FiId, GetQuoteRequest, LockedIssuanceRequest, OfferEpoch,
    PaymentTerms, Plan, QuoteId, QuoteTerms, QuoteTermsError, RefundIssuance, RefundTransaction,
    SeatId,
};

fn paid_request() -> GetQuoteRequest {
    let secret = secp256k1::SecretKey::from_slice(&[9; 32]).expect("valid scalar");
    GetQuoteRequest {
        fi_id: FiId(secret.x_only_public_key(secp256k1::SECP256K1).0),
        fedimintd_version: "0.0.0-test".parse().expect("valid test version"),
        federation_size: FederationSize(7),
        plan: Plan::InfiniteBestEffort { price_msats: 5 },
        payment_federation_id: Some(FederationId("fed".to_owned())),
        refund_issuance: Some(RefundIssuance::MintV1 {
            refund_nonce: [7; 32],
            issuance: vec![],
        }),
    }
}

/// Synthetic mint-v1 terms: coherence never decodes the blind nonces.
fn payment(amounts_msats: &[u64]) -> PaymentTerms {
    PaymentTerms::MintV1 {
        federation_id: FederationId("fed".to_owned()),
        issuance: amounts_msats
            .iter()
            .map(|&amount_msats| LockedIssuanceRequest {
                amount_msats,
                blind_nonce: vec![7; 48],
            })
            .collect(),
    }
}

#[test]
fn composed_paid_terms_carry_issuance() {
    let terms = QuoteTerms::compose(
        paid_request(),
        OfferEpoch::from_bytes([0; 32]),
        5,
        [3; 32],
        Some(payment(&[4, 1])),
    )
    .unwrap();
    terms.check_coherent().unwrap();
}

#[test]
fn composed_zero_price_terms_have_no_payment() {
    let mut request = paid_request();
    request.plan = Plan::InfiniteBestEffort { price_msats: 0 };
    request.payment_federation_id = None;
    request.refund_issuance = None;
    let terms =
        QuoteTerms::compose(request, OfferEpoch::from_bytes([0; 32]), 0, [0; 32], None).unwrap();
    assert!(terms.payment.is_none());
}

#[test]
fn incoherent_terms_are_refused() {
    // Free price with payment terms attached.
    assert!(matches!(
        QuoteTerms::compose(
            paid_request(),
            OfferEpoch::from_bytes([0; 32]),
            0,
            [5; 32],
            Some(payment(&[4]))
        ),
        Err(QuoteTermsError::PaymentMismatch)
    ));
    // Issuance that does not add up to the price.
    assert!(matches!(
        QuoteTerms::compose(
            paid_request(),
            OfferEpoch::from_bytes([0; 32]),
            5,
            [5; 32],
            Some(payment(&[4]))
        ),
        Err(QuoteTermsError::PriceIssuanceMismatch)
    ));
    // A paid quote naming a federation the request did not choose.
    let mut request = paid_request();
    request.payment_federation_id = Some(FederationId("other".to_owned()));
    assert!(matches!(
        QuoteTerms::compose(
            request,
            OfferEpoch::from_bytes([0; 32]),
            5,
            [5; 32],
            Some(payment(&[4, 1]))
        ),
        Err(QuoteTermsError::PaymentMismatch)
    ));
}

#[test]
fn refund_transaction_debug_is_redacted() {
    let debug = format!("{:?}", RefundTransaction(b"secret refund bytes".to_vec()));
    assert_eq!(debug, "RefundTransaction(\"<redacted>\")");
    assert!(!debug.contains("secret"));
}

#[test]
fn seat_id_is_the_quote_ids_canonical_hex_encoding() {
    let quote_id = QuoteId([0xab; 32]);
    let seat_id = SeatId::from(quote_id);
    assert_eq!(seat_id.quote_id(), quote_id);
    assert_eq!(seat_id.to_string(), "ab".repeat(32));
    assert_eq!(
        serde_json::to_string(&seat_id).unwrap(),
        format!("\"{}\"", "ab".repeat(32))
    );
    assert_eq!(
        serde_json::from_str::<SeatId>(&serde_json::to_string(&seat_id).unwrap()).unwrap(),
        seat_id
    );
    assert_eq!(seat_id.to_string().parse::<SeatId>().unwrap(), seat_id);

    for bad in ["", "ab", &"AB".repeat(32), &"x".repeat(64)] {
        assert!(
            bad.parse::<SeatId>().is_err(),
            "expected InvalidSeatId for {bad:?}"
        );
    }
}
