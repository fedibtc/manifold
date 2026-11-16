//! The bundled 0.13 `fedimintd` line
//! (ARCH-manifold-guardian-seat-processes *Subprocess model*).
//!
//! The MG spawns this binary, installed beside its own, for seats on the 0.13
//! line, with the same environment, control socket and argv as the 0.12
//! fedimintd it links itself. It registers the same module set: the upstream
//! modules plus the Manifold SPv2 initializer.

use std::path::PathBuf;

use fedimint_core::Amount;
use fedimint_server_core::ServerModuleInitRegistry;
use stability_pool_server::StabilityPoolInit;
use stability_pool_server::common::config::{CollateralRatio, OracleConfig};

#[cfg(not(any(target_env = "msvc", target_os = "ios", target_os = "android")))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

/// The same built-in jemalloc options as the MG binary's own fedimintd: see
/// `MALLOC_CONF` in `crates/mg/bin/src/main.rs`.
#[cfg(target_os = "linux")]
#[used]
#[unsafe(export_name = "malloc_conf")]
static MALLOC_CONF: Option<&std::ffi::c_char> = Some(unsafe {
    &*c"background_thread:true,dirty_decay_ms:1000,muzzy_decay_ms:0,prof:true,prof_active:false"
        .as_ptr()
});

/// Private parent-to-child path for the safe-event journal
/// (`mg_core::seat_process::SAFE_EVENT_DIR_ENV`).
const SAFE_EVENT_DIR_ENV: &str = "MG_SAFE_EVENT_DIR";

fn manifold_stability_pool_init() -> StabilityPoolInit {
    let (provider, seeker) = mg_spv2_params::COLLATERAL_RATIO;
    StabilityPoolInit {
        oracle_config: if mg_spv2_params::use_mock_oracle() {
            OracleConfig::Mock
        } else {
            OracleConfig::Aggregate
        },
        cycle_duration: mg_spv2_params::CYCLE_DURATION,
        collateral_ratio: CollateralRatio { provider, seeker },
        min_allowed_seek: Amount::from_msats(mg_spv2_params::MIN_SEEK_MSATS),
        min_allowed_provide: Amount::from_msats(mg_spv2_params::MIN_PROVIDE_MSATS),
        max_allowed_provide_fee_rate_ppb: mg_spv2_params::MAX_PROVIDER_FEE_RATE_PPB,
        min_allowed_cancellation_bps: mg_spv2_params::MIN_CANCELLATION_BPS,
    }
}

fn manifold_modules() -> ServerModuleInitRegistry {
    let mut modules = fedimintd::default_modules();
    modules.attach(manifold_stability_pool_init());
    modules
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let safe_event_layer = std::env::var_os(SAFE_EVENT_DIR_ENV).and_then(|directory| {
        match safe_tracing::layer(PathBuf::from(directory)) {
            Ok(layer) => Some(layer),
            Err(error) => {
                eprintln!("safe-event journal unavailable; continuing without it: {error}");
                None
            }
        }
    });
    let never = if let Some(layer) = safe_event_layer {
        fedimintd::run_with_extra_logging_layer(
            manifold_modules(),
            fedimint_core::fedimint_build_code_version_env!(),
            Some("fedi"),
            layer,
        )
        .await?
    } else {
        fedimintd::run(
            manifold_modules(),
            fedimint_core::fedimint_build_code_version_env!(),
            Some("fedi"),
        )
        .await?
    };
    match never {}
}
