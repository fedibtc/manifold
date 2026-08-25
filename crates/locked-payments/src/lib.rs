//! The key-locked ecash payment protocol shared by both of its roles.
//!
//! An FI (payer) funds mint outputs whose blinded nonces were derived by an
//! FMan (payee); a refusal is clawed back through a refund transaction whose
//! outputs the payer committed to in advance. Everything the two programs
//! must compute identically to stay in agreement lives here, and only here:
//! the per-generation note cryptography ([`locked_payment`] for mint v1,
//! [`locked_payment_v2`]), the quote/refund denomination selection
//! ([`denominations`]), and the refund preparation the payer commits with
//! and the payee re-runs as its validation oracle ([`refund`]).
//!
//! This crate holds no wallet state. Each role brings its own Fedimint
//! client and secret material: the FMan side is `fman-fedimint`, the FI
//! side is `fi-cli`'s wallet, and neither depends on the other.

pub mod denominations;
pub mod locked_payment;
pub mod locked_payment_v2;
pub mod refund;

use anyhow::Context as _;
use fedimint_client::ClientHandleArc;
use fedimint_client_module::secret::{DeriveableSecretClientExt as _, get_default_client_secret};
use fedimint_core::config::FederationId;
use fedimint_core::core::ModuleInstanceId;
use fedimint_derive_secret::DerivableSecret;
use fedimint_mintv2_client::MintClientModule as MintV2ClientModule;

/// Reproduce the module root passed by fedimint-client when opened with
/// `RootSecret::StandardDoubleDerive`, so notes derived outside the client
/// remain recoverable through the standard derivation path.
pub fn standard_module_root_secret(
    global_root_secret: &DerivableSecret,
    federation_id: FederationId,
    module_instance_id: ModuleInstanceId,
) -> DerivableSecret {
    get_default_client_secret(global_root_secret, &federation_id)
        .federation_key(&federation_id)
        .derive_module_secret(module_instance_id)
}

/// Resolve a client's mint-v2 module instance by the exact instance id
/// carried in quote terms.
pub fn mint_v2_module(
    client: &ClientHandleArc,
    module_instance_id: ModuleInstanceId,
) -> anyhow::Result<&MintV2ClientModule> {
    client
        .get_module_client_dyn(module_instance_id)?
        .as_any()
        .downcast_ref::<MintV2ClientModule>()
        .context("selected module is not mint-v2")
}
