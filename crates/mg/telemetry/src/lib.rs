//! MG telemetry transport and enrollment boundary.

mod enrollment;
mod rpc;

pub use enrollment::{TelemetryRegistrationWorkerHandle, start_registration};
pub use rpc::{GuardianTelemetryRpc, read_safe_event_journal};
