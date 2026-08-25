//! Reservation-less key-locked ecash payment messages.

use std::fmt;
use std::str::FromStr;

use crate::{
    FederationId, FederationSize, FedimintdVersion, FiId, Plan, SignedResponse, Timestamp,
};

/// Content-derived identity of a signed quote.
#[derive(
    serde::Deserialize, serde::Serialize, Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd,
)]
pub struct QuoteId(pub [u8; 32]);

/// Durable identity of the seat accepted for a quote.
///
/// This distinct type marks the quote's transition into an accepted seat while
/// making the identity relationship unrepresentable incorrectly: its wire and
/// display form is the quote id's 64-character lowercase hex encoding.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SeatId(QuoteId);

impl SeatId {
    pub const LEN: usize = 64;

    pub fn new(id: impl Into<String>) -> Result<Self, InvalidSeatId> {
        let id = id.into();
        let bytes = hex::decode(&id)
            .ok()
            .filter(|bytes| id.len() == Self::LEN && hex::encode(bytes) == id)
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or_else(|| InvalidSeatId { id: id.clone() })?;
        Ok(Self(QuoteId(bytes)))
    }

    pub fn quote_id(&self) -> QuoteId {
        self.0
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0.0
    }
}

impl From<QuoteId> for SeatId {
    fn from(quote_id: QuoteId) -> Self {
        Self(quote_id)
    }
}

impl FromStr for SeatId {
    type Err = InvalidSeatId;

    fn from_str(id: &str) -> Result<Self, Self::Err> {
        Self::new(id)
    }
}

impl serde::Serialize for SeatId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl fmt::Display for SeatId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex::encode(self.0.0))
    }
}

impl<'de> serde::Deserialize<'de> for SeatId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let id = String::deserialize(deserializer)?;
        Self::new(id).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, thiserror::Error)]
#[error(
    "invalid seat id {id:?}: must be a {}-character lowercase hex quote id",
    SeatId::LEN
)]
pub struct InvalidSeatId {
    pub id: String,
}

/// One blinded mint-v1 issuance output.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct LockedIssuanceRequest {
    pub amount_msats: u64,
    pub blind_nonce: Vec<u8>,
}

/// One blinded mint-v2 issuance output.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct LockedIssuanceRequestV2 {
    pub amount_msats: u64,
    pub blind_nonce: Vec<u8>,
    // review: wondering if we can merge LockedIssuananceRequest and V2 by just ignore tweak if mintv1 is used, we can just remove a lot of branching?
    /// Public mint-v2 output tweak.
    pub tweak: [u8; 16],
}

/// Aggregate blinded mint signature for one quoted issuance output.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct LockedBlindedSignature(pub Vec<u8>);

/// Consensus-encoded, fully signed fedimint refund transaction.
#[derive(serde::Deserialize, serde::Serialize, Clone, Eq, PartialEq)]
pub struct RefundTransaction(pub Vec<u8>);

impl fmt::Debug for RefundTransaction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("RefundTransaction")
            .field(&"<redacted>")
            .finish()
    }
}

/// Pure, unsigned quote request.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct GetQuoteRequest {
    pub fi_id: FiId,
    pub fedimintd_version: FedimintdVersion,
    pub federation_size: FederationSize,
    pub plan: Plan,
    /// Required for paid plans and absent for free plans.
    pub payment_federation_id: Option<FederationId>,
    /// Required for paid plans and absent for free plans.
    pub refund_issuance: Option<RefundIssuance>,
}

#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RefundIssuance {
    MintV1 {
        refund_nonce: [u8; 32],
        issuance: Vec<LockedIssuanceRequest>,
    },
    MintV2 {
        refund_nonce: [u8; 32],
        issuance: Vec<LockedIssuanceRequestV2>,
    },
}

/// Payment terms of a paid quote. The mint generation is never negotiated:
/// the payment federation's own modules decide it — mintv2 if the federation
/// has a mintv2 module, mintv1 otherwise — and the module instance is the
/// federation's first module of that kind. The FI learns the FMan's choice
/// from this variant and rejects the quote if it disagrees about the
/// federation's modules.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PaymentTerms {
    MintV1 {
        federation_id: FederationId,
        issuance: Vec<LockedIssuanceRequest>,
    },
    MintV2 {
        federation_id: FederationId,
        issuance: Vec<LockedIssuanceRequestV2>,
    },
}

/// Which locked-payment protocol a payment settles under.
///
/// A quote's [`PaymentTerms`] name the generation that was priced; a wallet
/// states the one it actually settled against. Naming it separately is what
/// lets the two be compared before any signature travels, so a wallet that
/// paid under the wrong protocol is caught by the payer rather than by the
/// FMan refusing the presentation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MintGeneration {
    MintV1,
    MintV2,
}

impl PaymentTerms {
    pub fn generation(&self) -> MintGeneration {
        match self {
            Self::MintV1 { .. } => MintGeneration::MintV1,
            Self::MintV2 { .. } => MintGeneration::MintV2,
        }
    }

    pub fn federation_id(&self) -> &FederationId {
        match self {
            Self::MintV1 { federation_id, .. } | Self::MintV2 { federation_id, .. } => {
                federation_id
            }
        }
    }

    /// Checked sum of the issuance set; the quoted price must equal it.
    pub fn total_msats(&self) -> Option<u64> {
        match self {
            Self::MintV1 { issuance, .. } => issuance
                .iter()
                .try_fold(0u64, |sum, output| sum.checked_add(output.amount_msats)),
            Self::MintV2 { issuance, .. } => issuance
                .iter()
                .try_fold(0u64, |sum, output| sum.checked_add(output.amount_msats)),
        }
    }
}

/// Random identifier for one version of the offer being quoted.
#[derive(serde::Deserialize, serde::Serialize, Clone, Copy, Debug, Eq, PartialEq)]
pub struct OfferEpoch([u8; 32]);

impl OfferEpoch {
    /// Wraps persisted or test bytes; FMan issuance uses fresh CSPRNG bytes.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The opaque bytes for persistence.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Public terms authenticated by the quote signature.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct QuoteTerms {
    /// Fresh randomness making every quote's issuance set unique even when
    /// all requested commercial terms and timestamps are identical.
    pub quote_nonce: [u8; 32],
    /// Offer epoch current when this quote was issued.
    pub offer_epoch: OfferEpoch,
    pub request: GetQuoteRequest,
    pub price_msats: u64,
    /// Present for paid quotes and absent for free quotes.
    pub payment: Option<PaymentTerms>,
}

/// A violation of the quote coherence rule ([`QuoteTerms::check_coherent`]).
#[derive(Debug, thiserror::Error)]
pub enum QuoteTermsError {
    #[error("paid-ness of the price, request, and payment terms disagree")]
    PaymentMismatch,
    #[error("the quoted price does not equal the issuance total")]
    PriceIssuanceMismatch,
    #[error("refund issuance presence or mint generation disagrees with payment terms")]
    RefundMismatch,
}

impl QuoteTerms {
    /// Compose coherent terms — the FMan side of quoting. Policy checks
    /// (offer, version, size, accepted federation) happen before this
    /// call; a paid quote's payment terms come from the wallet, keyed by
    /// `quote_nonce`. The terms get their identity when they are signed:
    /// the quote id is the hash of the signed response bytes.
    pub fn compose(
        request: GetQuoteRequest,
        offer_epoch: OfferEpoch,
        price_msats: u64,
        quote_nonce: [u8; 32],
        payment: Option<PaymentTerms>,
    ) -> Result<Self, QuoteTermsError> {
        let terms = Self {
            quote_nonce,
            offer_epoch,
            request,
            price_msats,
            payment,
        };
        terms.check_coherent()?;
        Ok(terms)
    }

    /// The coherence rule both protocol sides hold over one set of terms:
    /// a quote is paid iff it carries a price, its payment federation is
    /// exactly the requested one, and the price equals the issuance total
    /// (the FI funds the issuance set, so a mismatch would over- or
    /// under-charge). The FMan composes terms through this rule; the FI
    /// re-checks a verified quote against it before paying.
    // review: check coherent must be only way to construct this struct! custom serde deserialize if needed
    // review: maybe CoherentQuoteTerms wrapper QuoteTerms? and we can even remove compose above, just make it yourself with struct fields
    pub fn check_coherent(&self) -> Result<(), QuoteTermsError> {
        let consistent = match &self.payment {
            None => self.price_msats == 0 && self.request.payment_federation_id.is_none(),
            Some(terms) => {
                self.price_msats > 0
                    && self.request.payment_federation_id.as_ref() == Some(terms.federation_id())
            }
        };
        if !consistent {
            return Err(QuoteTermsError::PaymentMismatch);
        }
        if let Some(terms) = &self.payment
            && terms.total_msats() != Some(self.price_msats)
        {
            return Err(QuoteTermsError::PriceIssuanceMismatch);
        }
        let refund_matches = matches!(
            (&self.payment, &self.request.refund_issuance),
            (None, None)
                | (
                    Some(PaymentTerms::MintV1 { .. }),
                    Some(RefundIssuance::MintV1 { .. })
                )
                | (
                    Some(PaymentTerms::MintV2 { .. }),
                    Some(RefundIssuance::MintV2 { .. })
                )
        );
        if !refund_matches {
            return Err(QuoteTermsError::RefundMismatch);
        }
        Ok(())
    }
}

/// Signed quote response: the terms and nothing else. A paid quote's
/// per-note secrets rederive on the FMan from its wallet root and the
/// public terms, so nothing is sealed or escrowed; the quote's identity is
/// [`SignatureVerified::quote_id`](crate::SignatureVerified::quote_id), the
/// hash of these bytes as signed.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct GetQuoteResponse {
    pub terms: QuoteTerms,
}

impl crate::SignatureVerified<GetQuoteResponse> {
    /// The quote's identity: SHA-256 of the signed response payload — the
    /// exact bytes the FMan signed and the FI verified, so no
    /// canonicalization scheme is needed. One definition for every
    /// consumer: the FMan's idempotency index, the
    /// wallet's refund-transaction nonce, and any FI bookkeeping key on
    /// the same bytes. Living on the verified wrapper, an id can only ever
    /// name a quote whose signature was checked.
    pub fn quote_id(&self) -> QuoteId {
        QuoteId(self.payload_sha256())
    }
}

/// The only allocating FI verb.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct CreateSeatRequest {
    pub ts: Timestamp,
    pub fi_id: FiId,
    pub quote: SignedResponse<GetQuoteResponse>,
    /// Aggregate blinded signatures over the quote's issuance outputs, and the
    /// only thing this request adds to the quote it presents. Empty for a free
    /// quote.
    ///
    /// There is no separate payment *kind*: the quote's [`PaymentTerms`] already
    /// say which mint protocol was priced, so a presented kind could only ever
    /// agree with it or be rejected. Carrying just the signatures makes the
    /// disagreement unrepresentable instead of checked.
    pub payment_signatures: Vec<LockedBlindedSignature>,
}

#[derive(serde::Deserialize, serde::Serialize, Clone, Copy, Debug, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RefusalReason {
    OfferChanged,
}

#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CreateSeatOutcome {
    Accepted {
        seat_id: SeatId,
        /// This accepted seat's full guardian-fee remittance account.
        ///
        /// It is FMan-signed with the acceptance and replayed from the same
        /// deterministic seat id, so the FI can persist it before DKG without
        /// trusting a later unauthenticated discovery value.
        guardian_fee_account: crate::GuardianFeeAccount,
    },
    Refused {
        reason: RefusalReason,
        /// Present for paid quotes and absent for free quotes.
        refund_transaction: Option<RefundTransaction>,
    },
}

/// Signed acceptance/refusal commitment, bound to the quote it answers.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct CreateSeatResponse {
    pub quote_id: QuoteId,
    pub outcome: CreateSeatOutcome,
}

#[cfg(test)]
mod tests;
