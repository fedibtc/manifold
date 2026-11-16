//! The durable seat vocabulary: the facts SQLite records and every layer
//! above reads — creation facts and the seat's port addressing. This module sits at the bottom of the
//! import stack (ARCH-manifold-guardian *Module responsibilities*) so `db` can
//! persist these shapes and `seat`/`fleet` can live them without any
//! upward import.

use fedi_decentralized_service_manifold_guardian::{
    FaId, FederationSize, FedimintdVersion, GuardianCode, Plan, SeatId,
};

/// The immutable creation-time facts of a seat. They are inserted together
/// after payment verification and readable without a lock once loaded into
/// the in-memory registry. This struct contains only public lifecycle facts;
/// typed payment evidence lives in its own row. Preserve that
/// property when adding fields here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeatFacts {
    pub seat_id: SeatId,
    /// Never-reused local allocation ordinal; names the data directory and port block.
    pub seat_no: SeatNo,
    /// Identity binding for every later FA call.
    pub fa_id: FaId,
    /// Commercial terms the operator still reports after admission.
    pub plan: Plan,
    /// Guardian count used by DKG validation and leader setup.
    pub federation_size: FederationSize,
    pub created_at_ms: i64,
}

/// A bundled `fedimintd` major/minor line
/// (ARCH-manifold-guardian-seat-processes *Subprocess model*).
///
/// A release hosts every line listed here, and each seat runs exactly one.
/// A seat's `fedimintd` database is only readable by its own line and newer
/// ones, so a seat never moves to an older line.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum FedimintdLine {
    /// Linked into the MG binary and entered through the `fedimintd` argv[0].
    V0_12,
    /// The sibling `fedimintd-0-13` binary from the same release.
    V0_13,
}

impl FedimintdLine {
    /// The line this release forms new seats on, and the only one it
    /// advertises and quotes (SPEC-fa-rpc release policy).
    pub const FORMATION: Self = Self::V0_12;

    /// Whether this is the line documents from before lines existed imply.
    pub fn is_v0_12(&self) -> bool {
        *self == Self::V0_12
    }

    /// Durable spelling, shared by SQLite and the backup document.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::V0_12 => "0.12",
            Self::V0_13 => "0.13",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "0.12" => Some(Self::V0_12),
            "0.13" => Some(Self::V0_13),
            _ => None,
        }
    }

    /// The `fedimintd` version this line advertises and accepts in quotes.
    pub fn version(self) -> FedimintdVersion {
        match self {
            Self::V0_12 => "0.12.0+fedi",
            Self::V0_13 => "0.13.0+fedi",
        }
        .parse()
        .expect("a line's version literal is valid SemVer")
    }
}

impl serde::Serialize for FedimintdLine {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for FedimintdLine {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::from_str(&value).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "fedimintd line {value:?} is not hosted by this release"
            ))
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SeatNo(pub u32);

impl SeatNo {
    /// The only conversion from a durable ordinal to its four-port block.
    pub fn port_base(self, first: PortBase) -> Option<PortBase> {
        let offset = u16::try_from(self.0).ok()?.checked_mul(SeatPorts::BLOCK)?;
        PortBase::new(first.get().checked_add(offset)?)
    }
}

/// Durable callback delivery state. Bearer material is deliberately absent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CompletionCallbackStatus {
    NotConfigured,
    Pending {
        attempts: u32,
        next_attempt_at_ms: i64,
        last_reason: Option<CompletionCallbackReason>,
    },
    OperatorBlocked {
        attempts: u32,
        reason: CompletionCallbackReason,
    },
    Delivered {
        attempts: u32,
        at_ms: i64,
    },
    Terminal {
        attempts: u32,
        at_ms: i64,
        reason: CompletionCallbackReason,
    },
}

impl CompletionCallbackStatus {
    pub(crate) fn attempts(&self) -> u32 {
        match self {
            Self::NotConfigured => 0,
            Self::Pending { attempts, .. }
            | Self::OperatorBlocked { attempts, .. }
            | Self::Delivered { attempts, .. }
            | Self::Terminal { attempts, .. } => *attempts,
        }
    }
}

/// Low-cardinality callback result reasons safe for logs and operator output.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompletionCallbackReason {
    GatewayOriginMissing,
    GatewayOriginMismatch,
    HttpClientUnavailable,
    Network,
    GatewayUnavailable,
    /// The gateway holds the phone: the provider rejected its token, and the
    /// app has to be opened (and re-register) before alerts reach it again.
    TargetInactive,
    RateLimited,
    HookNotFound,
    HookExpiredOrRevoked,
    MaxUsesExceeded,
    PolicyRejected,
    Decommissioned,
    Superseded,
}

impl CompletionCallbackReason {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::GatewayOriginMissing => "gateway_origin_missing",
            Self::GatewayOriginMismatch => "gateway_origin_mismatch",
            Self::HttpClientUnavailable => "http_client_unavailable",
            Self::Network => "network",
            Self::GatewayUnavailable => "gateway_unavailable",
            Self::TargetInactive => "target_inactive",
            Self::RateLimited => "rate_limited",
            Self::HookNotFound => "hook_not_found",
            Self::HookExpiredOrRevoked => "hook_expired_or_revoked",
            Self::MaxUsesExceeded => "max_uses_exceeded",
            Self::PolicyRejected => "policy_rejected",
            Self::Decommissioned => "decommissioned",
            Self::Superseded => "superseded",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        Some(match value {
            "gateway_origin_missing" => Self::GatewayOriginMissing,
            "gateway_origin_mismatch" => Self::GatewayOriginMismatch,
            "http_client_unavailable" => Self::HttpClientUnavailable,
            "network" => Self::Network,
            "gateway_unavailable" => Self::GatewayUnavailable,
            "target_inactive" => Self::TargetInactive,
            "rate_limited" => Self::RateLimited,
            "hook_not_found" => Self::HookNotFound,
            "hook_expired_or_revoked" => Self::HookExpiredOrRevoked,
            "max_uses_exceeded" => Self::MaxUsesExceeded,
            "policy_rejected" => Self::PolicyRejected,
            "decommissioned" => Self::Decommissioned,
            "superseded" => Self::Superseded,
            _ => return None,
        })
    }
}

/// The canonical guardian code set a DKG session is started with. Valid by
/// construction: one only exists after the `StartDKG` input checks
/// passed (expected count, no duplicates, own code present), and it is
/// always in canonical (sorted) form, so the recorded-vs-retry comparison
/// can never be fooled by submission order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DkgCodeSet(Vec<GuardianCode>);

impl DkgCodeSet {
    pub fn validate(
        codes: &[GuardianCode],
        expected: FederationSize,
        own_code: &GuardianCode,
    ) -> Result<Self, DkgCodeSetError> {
        let expected = usize::from(expected.0);
        if codes.len() != expected {
            return Err(DkgCodeSetError::WrongCount {
                expected,
                got: codes.len(),
            });
        }
        let mut codes = codes.to_vec();
        codes.sort_by(|left, right| left.0.cmp(&right.0));
        if codes.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(DkgCodeSetError::DuplicateCode);
        }
        if !codes.contains(own_code) {
            return Err(DkgCodeSetError::OwnCodeMissing);
        }
        Ok(Self(codes))
    }

    pub fn iter(&self) -> impl Iterator<Item = &GuardianCode> {
        self.0.iter()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DkgCodeSetError {
    #[error("expected {expected} guardian codes, got {got}")]
    WrongCount { expected: usize, got: usize },

    #[error("duplicate guardian code")]
    DuplicateCode,

    #[error("own guardian code missing from the set")]
    OwnCodeMissing,
}

/// One seat's contiguous port block (p2p, api, ui, metrics), derived on
/// demand from its [`PortBase`] — which already proved the whole block fits
/// in `u16`, so every accessor is infallible.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SeatPorts(PortBase);

impl SeatPorts {
    /// Ports per seat: one contiguous block of p2p, api, ui, metrics.
    pub const BLOCK: u16 = 4;

    /// The base this block was built from.
    pub fn base(&self) -> PortBase {
        self.0
    }

    pub fn from_base(port_base: PortBase) -> Self {
        Self(port_base)
    }

    pub(crate) fn p2p(self) -> u16 {
        self.0.get()
    }

    /// The fedimintd API port, where [`crate::fedimint_api`] connects.
    pub fn api(self) -> u16 {
        self.0.get() + 1
    }

    pub(crate) fn ui(self) -> u16 {
        self.0.get() + 2
    }

    pub(crate) fn metrics(self) -> u16 {
        self.0.get() + 3
    }
}

/// A seat's first port, valid by construction: it only exists if the whole
/// [`SeatPorts::BLOCK`]-port block fits in `u16`, so every holder (the
/// allocator, a persisted row) can expand it to [`SeatPorts`] infallibly.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PortBase(u16);

impl PortBase {
    pub fn new(base: u16) -> Option<Self> {
        base.checked_add(SeatPorts::BLOCK - 1).map(|_| Self(base))
    }

    pub fn get(self) -> u16 {
        self.0
    }

    /// The following non-overlapping block, or `None` when the lifetime port
    /// grid is exhausted.
    pub fn next_block(self) -> Option<Self> {
        self.0.checked_add(SeatPorts::BLOCK).and_then(Self::new)
    }

    /// Number of complete non-overlapping blocks remaining on this cursor's
    /// `base + 4k` lifetime grid, including this block.
    pub fn remaining_blocks(self) -> u32 {
        let remaining_ports = u32::from(u16::MAX) - u32::from(self.0) + 1;
        remaining_ports / u32::from(SeatPorts::BLOCK)
    }
}

#[cfg(test)]
#[path = "../tests/facts.rs"]
mod tests;
