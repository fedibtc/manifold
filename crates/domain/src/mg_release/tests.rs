//! Tests for the threshold-signed MG release.

use super::*;

fn release(url: &str) -> MgRelease {
    MgRelease {
        source_commit: "0123456789abcdef0123456789abcdef01234567".parse().unwrap(),
        binaries: BTreeMap::from([
            (
                "x86_64".parse().unwrap(),
                MgReleaseBinary {
                    sha256: Sha256Hash::new([1; 32]),
                    size: 1000,
                    url: Url(url.to_owned()),
                },
            ),
            (
                "aarch64".parse().unwrap(),
                MgReleaseBinary {
                    sha256: Sha256Hash::new([2; 32]),
                    size: 2000,
                    url: Url("https://example.com/aarch64".to_owned()),
                },
            ),
        ]),
        signatures: Vec::new(),
    }
}

fn version(value: &str) -> MgVersion {
    value.parse().unwrap()
}

#[test]
fn signature_binds_environment_version_and_binaries() {
    let keys = nostr::Keys::generate();
    let mut signed = release("https://example.com/x86_64");
    signed
        .signatures
        .push(signed.sign("staging", &version("0.2.0"), &keys));

    let only_signer = BTreeSet::from([keys.public_key()]);
    assert_eq!(
        signed.valid_signers("staging", &version("0.2.0")),
        only_signer
    );
    // A staging signature must not authorize production or another version.
    assert!(
        signed
            .valid_signers("production", &version("0.2.0"))
            .is_empty()
    );
    assert!(
        signed
            .valid_signers("staging", &version("0.2.1"))
            .is_empty()
    );

    let mut tampered = signed.clone();
    tampered.binaries.get_mut("aarch64").unwrap().sha256 = Sha256Hash::new([3; 32]);
    assert!(
        tampered
            .valid_signers("staging", &version("0.2.0"))
            .is_empty()
    );

    let mut moved = signed.clone();
    moved.binaries.get_mut("x86_64").unwrap().url = Url("https://evil.example/x".to_owned());
    assert!(moved.valid_signers("staging", &version("0.2.0")).is_empty());
}

#[test]
fn repeated_and_forged_signatures_count_once_or_not_at_all() {
    let first = nostr::Keys::generate();
    let second = nostr::Keys::generate();
    let mut signed = release("https://example.com/x86_64");
    let signature = signed.sign("staging", &version("0.2.0"), &first);
    signed.signatures.push(signature.clone());
    signed.signatures.push(signature.clone());
    // Second's key claimed with first's signature: does not verify.
    signed.signatures.push(MgReleaseSignature {
        signer: second.public_key(),
        signature: signature.signature,
    });

    assert_eq!(
        signed.valid_signers("staging", &version("0.2.0")),
        BTreeSet::from([first.public_key()])
    );
}

#[test]
fn json_round_trips_and_rejects_uppercase_hex() {
    let keys = nostr::Keys::generate();
    let mut signed = release("http://127.0.0.1:8080/fleet-manager");
    signed
        .signatures
        .push(signed.sign("development", &version("0.1.9"), &keys));
    let json = serde_json::to_string(&signed).unwrap();
    let parsed: MgRelease = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, signed);
    assert_eq!(
        parsed.valid_signers("development", &version("0.1.9")).len(),
        1
    );

    let upper = json.replace(
        "0123456789abcdef0123456789abcdef01234567",
        "0123456789ABCDEF0123456789abcdef01234567",
    );
    assert!(serde_json::from_str::<MgRelease>(&upper).is_err());
}

#[test]
fn validate_rejects_unusable_binaries_and_urls() {
    assert_eq!(release("https://example.com/x").validate(), Ok(()));
    assert_eq!(release("http://127.0.0.1:8080/x").validate(), Ok(()));

    for url in [
        "ftp://example.com/x",
        "https://user:pw@example.com/x",
        // Not in normalized form: the signed string must be the fetched one.
        "HTTPS://example.com/x",
        "not a url",
    ] {
        assert_eq!(
            release(url).validate(),
            Err(MgReleaseError::InvalidUrl),
            "{url}"
        );
    }

    let mut empty = release("https://example.com/x");
    empty.binaries.get_mut("x86_64").unwrap().size = 0;
    assert_eq!(empty.validate(), Err(MgReleaseError::BinarySize));

    let mut none = release("https://example.com/x");
    none.binaries.clear();
    assert_eq!(none.validate(), Err(MgReleaseError::BinaryCount));
}
