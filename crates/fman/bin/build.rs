//! Compile-time inputs for the shipped binary.

use std::path::PathBuf;

use fedimint_build::envs::{FEDIMINT_BUILD_CODE_VERSION_ENV, FORCE_GIT_HASH_ENV};

/// Git revision of the pinned Fedimint input (`flake.lock`). The bundled
/// fedimintd reports it as `version_hash` in `fm_app_start_ts`, and the
/// guardian metrics policy forwards that diagnostic, so plain `cargo` builds
/// must stamp the pinned source too — not this checkout's own commit, which is what
/// `fedimint_build::set_code_version` would read. `fleetManagerReleaseSync` in
/// `flake.nix` keeps it equal to the flake input.
const FEDIMINT_SOURCE_REV: &str = "a6fa6d83f4bea26d4f51cbf26d305d0b64727e00";

fn main() {
    // A packager can still override the stamp, as the Nix builds do.
    println!("cargo:rerun-if-env-changed={FORCE_GIT_HASH_ENV}");
    let hash = std::env::var(FORCE_GIT_HASH_ENV).unwrap_or_else(|_| FEDIMINT_SOURCE_REV.to_owned());
    println!("cargo:rustc-env={FEDIMINT_BUILD_CODE_VERSION_ENV}={hash}");

    println!("cargo:rerun-if-env-changed=FMAN_OPERATOR_UI_DIST_DIR");
    if std::env::var_os("CARGO_FEATURE_EMBEDDED_OPERATOR_UI").is_none() {
        return;
    }

    let dist = PathBuf::from(
        std::env::var_os("FMAN_OPERATOR_UI_DIST_DIR")
            .expect("embedded-operator-ui requires FMAN_OPERATOR_UI_DIST_DIR"),
    );
    assert!(
        dist.join("index.html").is_file(),
        "FMAN_OPERATOR_UI_DIST_DIR does not contain index.html: {}",
        dist.display()
    );
    assert!(
        dist.join("assets").is_dir(),
        "FMAN_OPERATOR_UI_DIST_DIR does not contain assets/: {}",
        dist.display()
    );
    println!("cargo:rerun-if-changed={}", dist.display());
}
