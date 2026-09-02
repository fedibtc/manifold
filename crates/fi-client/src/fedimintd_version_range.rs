//! FI-approved Fedimint release ranges and vendor policy.

use fedi_decentralized_service_fleet_manager::{
    FEDIMINTD_VENDOR_0_1, FedimintdDkgVersion, FedimintdVersion, FedimintdVersionCore,
};

use crate::{FiError, FiResult};

/// FI-approved half-open range of three-number Fedimint releases and exact
/// vendor policy.
///
/// Prerelease and build metadata are intentionally outside these bounds. This
/// policy controls which exact releases the FI accepts; DKG compatibility is
/// separately based on major/minor/vendor and may span patches in the range.
/// The current constructor and serialized schema require `vendor = "fedi"`;
/// that check is localized here so a later vendor-policy change need not alter
/// range or cohort semantics.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(try_from = "UncheckedFedimintdVersionRange")]
pub struct FedimintdVersionRange {
    minimum: FedimintdVersionCore,
    maximum_exclusive: FedimintdVersionCore,
    vendor: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct UncheckedFedimintdVersionRange {
    minimum: FedimintdVersionCore,
    maximum_exclusive: FedimintdVersionCore,
    vendor: String,
}

impl TryFrom<UncheckedFedimintdVersionRange> for FedimintdVersionRange {
    type Error = FiError;

    fn try_from(value: UncheckedFedimintdVersionRange) -> FiResult<Self> {
        Self::from_cores(value.minimum, value.maximum_exclusive, value.vendor)
    }
}

impl FedimintdVersionRange {
    /// Construct `[minimum, maximum_exclusive)` from two Fedimint versions.
    ///
    /// Prerelease suffixes on the bounds are ignored. Both bounds must carry
    /// the currently accepted exact `+fedi` vendor identity; accepting another
    /// vendor is a separate policy decision from the range mechanism.
    pub fn new(minimum: FedimintdVersion, maximum_exclusive: FedimintdVersion) -> FiResult<Self> {
        if !minimum.dkg_version().is_fedi() || !maximum_exclusive.dkg_version().is_fedi() {
            return Err(FiError::InvalidIntent(format!(
                "fedimintd version range bounds must use +{FEDIMINTD_VENDOR_0_1}"
            )));
        }
        Self::from_cores(
            minimum.core(),
            maximum_exclusive.core(),
            FEDIMINTD_VENDOR_0_1.to_owned(),
        )
    }

    fn from_cores(
        minimum: FedimintdVersionCore,
        maximum_exclusive: FedimintdVersionCore,
        vendor: String,
    ) -> FiResult<Self> {
        let range = Self {
            minimum,
            maximum_exclusive,
            vendor,
        };
        range.validate()?;
        Ok(range)
    }

    fn validate(&self) -> FiResult<()> {
        if self.minimum >= self.maximum_exclusive {
            return Err(FiError::InvalidIntent(
                "fedimintd version range must have a lower minimum than maximum".to_owned(),
            ));
        }
        if self.vendor != FEDIMINTD_VENDOR_0_1 {
            return Err(FiError::InvalidIntent(format!(
                "fedimintd version range vendor must be {FEDIMINTD_VENDOR_0_1}"
            )));
        }
        Ok(())
    }

    /// Range containing exactly one patch release.
    pub fn one_core(core: FedimintdVersionCore) -> FiResult<Self> {
        let maximum_exclusive = FedimintdVersionCore {
            major: core.major,
            minor: core.minor,
            patch: core.patch.checked_add(1).ok_or_else(|| {
                FiError::InvalidIntent("fedimintd release patch cannot be ranged".to_owned())
            })?,
        };
        Self::from_cores(core, maximum_exclusive, FEDIMINTD_VENDOR_0_1.to_owned())
    }

    /// Return the sole patch release when this range contains exactly one.
    #[must_use]
    pub fn only_core(&self) -> Option<FedimintdVersionCore> {
        Self::one_core(self.minimum)
            .ok()
            .filter(|single| single.maximum_exclusive == self.maximum_exclusive)
            .map(|_| self.minimum)
    }

    /// Inclusive lower release bound.
    #[must_use]
    pub fn minimum(&self) -> FedimintdVersionCore {
        self.minimum
    }

    /// Exclusive upper release bound.
    #[must_use]
    pub fn maximum_exclusive(&self) -> FedimintdVersionCore {
        self.maximum_exclusive
    }

    /// Whether one exact FMan build lies inside this release range.
    #[must_use]
    pub fn contains(&self, version: &FedimintdVersion) -> bool {
        version.dkg_version().vendor() == Some(self.vendor.as_str())
            && self.contains_core(version.core())
    }

    /// Whether one three-number release lies inside this range.
    #[must_use]
    pub fn contains_core(&self, core: FedimintdVersionCore) -> bool {
        self.minimum <= core && core < self.maximum_exclusive
    }

    /// Whether this range contains any patch from one DKG major/minor line.
    #[must_use]
    pub fn overlaps_dkg(&self, dkg: &FedimintdDkgVersion) -> bool {
        if dkg.vendor() != Some(self.vendor.as_str()) {
            return false;
        }
        let line = dkg.major_minor();
        let minimum_line = (self.minimum.major, self.minimum.minor);
        let maximum_line = (self.maximum_exclusive.major, self.maximum_exclusive.minor);
        minimum_line <= line
            && (line < maximum_line || (line == maximum_line && self.maximum_exclusive.patch > 0))
    }
}
