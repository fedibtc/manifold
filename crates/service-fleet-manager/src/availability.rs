//! Availability messages.

use crate::{AvailabilityInfo, FederationSize, FedimintdVersion, Plan};

/// Request for public Fleet Manager availability.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Default, Eq, PartialEq)]
pub struct GetAvailabilityRequest;

/// Public Fleet Manager availability.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct GetAvailabilityResponse {
    /// Whether this FM would allocate a seat right now. A boolean for the
    /// same reason the advertisement carries one: a live seat count is
    /// operator business, and any caller could poll for it.
    pub accepting_seats: bool,

    /// Exact Fedimint daemon version this FM hosts.
    pub fedimintd_version: FedimintdVersion,

    /// Federation sizes this FM will host. Like `fedimintd_version`, the set
    /// ships with the FM release rather than being an operator knob.
    pub federation_sizes: Vec<FederationSize>,

    /// Plans offered by this FM (each carries its own pricing and period
    /// terms).
    pub plans: Vec<Plan>,

    /// Other availability data useful to Fedi App.
    pub additional_info: Vec<AvailabilityInfo>,
}

#[cfg(test)]
mod tests;
