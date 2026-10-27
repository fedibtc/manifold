//! Tests for automatic MG updates.

use std::collections::BTreeMap;
use std::net::SocketAddr;

use fedi_decentralized_domain::{MgRelease, MgReleaseSignature, Url};
use fedi_decentralized_manifold_environment::DEV_MG_RELEASE_SIGNERS_ENV;

use super::*;

struct Signers {
    keys: Vec<nostr_sdk::Keys>,
    policy: MgReleaseSigners,
}

/// A 2-of-3 development signer set, built through the profile's own parser.
fn signers() -> Signers {
    let keys: Vec<_> = (0..3).map(|_| nostr_sdk::Keys::generate()).collect();
    let value = format!(
        "2:{}",
        keys.iter()
            .map(|key| key.public_key().to_string())
            .collect::<Vec<_>>()
            .join(",")
    );
    let profile = temp_env(DEV_MG_RELEASE_SIGNERS_ENV, &value, || {
        ManifoldEnvironment::Development.profile().unwrap()
    });
    Signers {
        keys,
        policy: profile.mg_release_signers().unwrap().clone(),
    }
}

fn temp_env<T>(name: &str, value: &str, f: impl FnOnce() -> T) -> T {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = LOCK.lock().unwrap();
    // SAFETY: serialized by LOCK; no other code in this test binary reads it.
    unsafe { std::env::set_var(name, value) };
    let result = f();
    unsafe { std::env::remove_var(name) };
    result
}

fn binary(bytes: &[u8], url: &str) -> MgReleaseBinary {
    MgReleaseBinary {
        sha256: Sha256Hash::new(Sha256::digest(bytes).into()),
        size: bytes.len() as u64,
        url: Url(url.to_owned()),
    }
}

fn release(arch: &str, binary: MgReleaseBinary) -> MgRelease {
    MgRelease {
        source_commit: "0123456789abcdef0123456789abcdef01234567".parse().unwrap(),
        binaries: BTreeMap::from([(arch.parse().unwrap(), binary)]),
        signatures: Vec::new(),
    }
}

fn signed(
    mut release: MgRelease,
    environment: &str,
    version: &str,
    keys: &[&nostr_sdk::Keys],
) -> MgRelease {
    let version = version.parse().unwrap();
    let signatures: Vec<MgReleaseSignature> = keys
        .iter()
        .map(|keys| release.sign(environment, &version, keys))
        .collect();
    release.signatures = signatures;
    release
}

fn policy(version: &str, release: Option<MgRelease>) -> AdmittedSetupPaymentFederations {
    let mut content = serde_json::json!({
        "version": 1,
        "fman_version": version,
        "federations": [],
        "telemetry_registration_url": "https://telemetry.example/v1/telemetry/registrations",
    });
    if let Some(release) = release {
        content["mg_release"] = serde_json::to_value(release).unwrap();
    }
    AdmittedSetupPaymentFederations::parse(content.to_string().as_bytes()).unwrap()
}

/// The running version with its patch number raised by one.
fn newer() -> String {
    let (head, patch) = env!("CARGO_PKG_VERSION").rsplit_once('.').unwrap();
    format!("{head}.{}", patch.parse::<u64>().unwrap() + 1)
}

#[test]
fn installs_only_a_newer_threshold_signed_release_for_this_cpu() {
    let signers = signers();
    let [a, b, c] = [&signers.keys[0], &signers.keys[1], &signers.keys[2]];
    let outsider = nostr_sdk::Keys::generate();
    let arch = std::env::consts::ARCH;
    let image = binary(b"new release", "https://example.com/fleet-manager");
    let newer = newer();
    let running = running_version();
    let decide_for = |policy: &AdmittedSetupPaymentFederations, staged: Option<&Staged>| {
        decide(
            policy,
            ManifoldEnvironment::Development,
            &signers.policy,
            &running,
            staged,
        )
    };

    let good = policy(
        &newer,
        Some(signed(
            release(arch, image.clone()),
            "development",
            &newer,
            &[a, c],
        )),
    );
    assert_eq!(
        decide_for(&good, None),
        Decision::Install {
            version: newer.parse().unwrap(),
            binary: image.clone()
        }
    );

    // One signer plus an outsider is below the 2-of-3 threshold, and two
    // copies of one signer's signature count once.
    for keys in [&[a, &outsider][..], &[b, b][..]] {
        let below = policy(
            &newer,
            Some(signed(
                release(arch, image.clone()),
                "development",
                &newer,
                keys,
            )),
        );
        assert!(matches!(decide_for(&below, None), Decision::Skip(_)));
    }

    // Signatures for another environment do not count.
    let staging = policy(
        &newer,
        Some(signed(
            release(arch, image.clone()),
            "staging",
            &newer,
            &[a, b],
        )),
    );
    assert!(matches!(decide_for(&staging, None), Decision::Skip(_)));

    // The running version, even fully signed, is not an update.
    let current = running.to_string();
    let same = policy(
        &current,
        Some(signed(
            release(arch, image.clone()),
            "development",
            &current,
            &[a, b, c],
        )),
    );
    assert!(matches!(decide_for(&same, None), Decision::Skip(_)));

    // No binary for this CPU.
    let other_arch = if arch == "x86_64" {
        "aarch64"
    } else {
        "x86_64"
    };
    let foreign = policy(
        &newer,
        Some(signed(
            release(other_arch, image.clone()),
            "development",
            &newer,
            &[a, b],
        )),
    );
    assert!(matches!(decide_for(&foreign, None), Decision::Skip(_)));

    assert!(matches!(
        decide_for(&policy(&newer, None), None),
        Decision::Skip(_)
    ));

    // Already staged and still running older code: the exec failed before.
    let staged = Staged {
        version: newer.parse().unwrap(),
        sha256: image.sha256,
    };
    assert!(matches!(
        decide_for(&good, Some(&staged)),
        Decision::Skip(_)
    ));
}

async fn serve_bytes(bytes: &'static [u8]) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = axum::Router::new().route(
        "/fleet-manager",
        axum::routing::get(move || async move { bytes }),
    );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    address
}

#[tokio::test]
async fn install_stages_only_the_published_bytes_and_launch_checks_them() {
    let served: &'static [u8] = b"#!/bin/sh\necho staged release\n";
    let address = serve_bytes(served).await;
    let url = format!("http://{address}/fleet-manager");
    let data = tempfile::tempdir().unwrap();
    let releases = Releases::new(data.path());
    let client = reqwest::Client::new();
    let version: MgVersion = newer().parse().unwrap();

    // Wrong hash, too small a published size, and too large a published
    // size all fail and stage nothing.
    let mut wrong_hash = binary(served, &url);
    wrong_hash.sha256 = Sha256Hash::new([7; 32]);
    let mut short = binary(served, &url);
    short.size -= 1;
    let mut long = binary(served, &url);
    long.size += 1;
    for bad in [wrong_hash, short, long] {
        assert!(releases.install(&version, &bad, &client).await.is_err());
        assert_eq!(releases.staged().unwrap(), None);
        assert_eq!(staged_binary(&releases).unwrap(), None);
    }

    let good = binary(served, &url);
    releases.install(&version, &good, &client).await.unwrap();
    let path = releases.binary_path(&good.sha256);
    assert_eq!(std::fs::read(&path).unwrap(), served);
    use std::os::unix::fs::PermissionsExt as _;
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert_eq!(staged_binary(&releases).unwrap(), Some(path.clone()));

    // A later change to the file is refused at launch.
    std::fs::write(&path, b"tampered").unwrap();
    assert!(staged_binary(&releases).is_err());
}

#[test]
fn launch_keeps_the_running_binary_unless_the_staged_one_is_newer() {
    let data = tempfile::tempdir().unwrap();
    let releases = Releases::new(data.path());
    std::fs::create_dir_all(&releases.dir).unwrap();
    let bytes = b"same version";
    let sha256 = Sha256Hash::new(Sha256::digest(bytes).into());
    std::fs::write(releases.binary_path(&sha256), bytes).unwrap();
    // A platform update brought a binary as new as the staged one (or newer):
    // the running binary wins.
    for version in [running_version().to_string(), "0.0.1".to_owned()] {
        let staged = Staged {
            version: version.parse().unwrap(),
            sha256,
        };
        write_atomically(
            &releases.dir,
            STAGED_FILE,
            &serde_json::to_vec(&staged).unwrap(),
        )
        .unwrap();
        assert_eq!(staged_binary(&releases).unwrap(), None, "{version}");
    }
}

#[test]
fn prune_keeps_only_the_staged_binary() {
    let data = tempfile::tempdir().unwrap();
    let releases = Releases::new(data.path());
    std::fs::create_dir_all(&releases.dir).unwrap();
    let keep = Sha256Hash::new([1; 32]);
    let old = Sha256Hash::new([2; 32]);
    for name in [keep.to_string(), old.to_string(), format!("{keep}.partial")] {
        std::fs::write(releases.dir.join(name), b"x").unwrap();
    }
    let staged = Staged {
        version: newer().parse().unwrap(),
        sha256: keep,
    };
    write_atomically(
        &releases.dir,
        STAGED_FILE,
        &serde_json::to_vec(&staged).unwrap(),
    )
    .unwrap();

    releases.prune();

    let mut left: Vec<String> = std::fs::read_dir(&releases.dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    left.sort();
    assert_eq!(left, vec![keep.to_string(), STAGED_FILE.to_owned()]);
}

#[test]
fn prune_drops_a_staged_release_that_a_platform_update_overtook() {
    let data = tempfile::tempdir().unwrap();
    let releases = Releases::new(data.path());
    std::fs::create_dir_all(&releases.dir).unwrap();
    let overtaken = Sha256Hash::new([3; 32]);
    std::fs::write(releases.dir.join(overtaken.to_string()), b"x").unwrap();
    // The platform shipped this very version; the staged binary is not running.
    let staged = Staged {
        version: running_version(),
        sha256: overtaken,
    };
    write_atomically(
        &releases.dir,
        STAGED_FILE,
        &serde_json::to_vec(&staged).unwrap(),
    )
    .unwrap();

    releases.prune();

    assert_eq!(std::fs::read_dir(&releases.dir).unwrap().count(), 0);
    assert_eq!(staged_binary(&releases).unwrap(), None);
}
