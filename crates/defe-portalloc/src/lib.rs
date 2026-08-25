//! A library for cooperative port allocation between multiple processes.
//!
//! `defe` tests need to allocate ranges of unused ports for local resources
//! without being able to bind them for the whole lifetime of a test.
//!
//! This library keeps track of allocated ports with an expiration timeout in a
//! shared file protected by an advisory filesystem lock, and uses TCP and UDP
//! binds to make sure a candidate port is actually free before reserving it.

mod data;
mod envs;
mod util;

use std::path::PathBuf;

use anyhow::bail;

use crate::data::DataDir;
pub use crate::envs::DEV_DEFE_PORTALLOC_DATA_DIR;
pub use crate::envs::DEV_DEFE_PORTALLOC_DATA_DIR as DEV_DEFE_PORTALLOC_DATA_DIR_ENV;

/// Allocate a contiguous range of IPv4 loopback ports.
///
/// The returned value is the first port in the allocated range. The allocation
/// is cooperative and short-lived: it prevents other users of this crate from
/// choosing the same range while the caller starts and binds its service.
pub fn port_alloc(range_size: u16) -> anyhow::Result<u16> {
    if range_size == 0 {
        bail!("can't allocate a range of zero ports");
    }

    let mut data_dir = DataDir::new(data_dir()?)?;

    data_dir.with_lock(|data_dir| {
        let mut data = data_dir.load_data()?;
        let base_port = data.get_free_port_range(range_size);
        data_dir.store_data(&data)?;
        Ok(base_port)
    })
}

fn data_dir() -> anyhow::Result<PathBuf> {
    if let Some(path) = std::env::var_os(DEV_DEFE_PORTALLOC_DATA_DIR) {
        Ok(PathBuf::from(path))
    } else if let Some(path) = dirs::cache_dir() {
        Ok(path.join("defe-portalloc"))
    } else {
        bail!(
            "could not determine port allocation data directory; set {DEV_DEFE_PORTALLOC_DATA_DIR}"
        );
    }
}
