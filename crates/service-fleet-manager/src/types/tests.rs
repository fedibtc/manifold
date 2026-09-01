use super::{FedimintdDkgVersion, FedimintdVersion, FedimintdVersionCore, MetaConsensusBase};

#[test]
fn fedimintd_version_is_semver_and_uses_string_serde() {
    let version = "0.11.1-fedi10"
        .parse::<FedimintdVersion>()
        .expect("valid release version");

    assert_eq!(version.to_string(), "0.11.1-fedi10");
    assert_eq!(
        serde_json::to_string(&version).expect("serialize version"),
        r#""0.11.1-fedi10""#
    );
    assert_eq!(
        serde_json::from_str::<FedimintdVersion>(r#""0.11.1-fedi10""#)
            .expect("deserialize version"),
        version
    );

    for invalid in ["", "test", "fedimintd-0.11.1-fedi10"] {
        assert!(invalid.parse::<FedimintdVersion>().is_err(), "{invalid}");
        assert!(
            serde_json::from_str::<FedimintdVersion>(&format!(r#""{invalid}""#)).is_err(),
            "{invalid}"
        );
    }
}

#[test]
fn fedimintd_version_separates_release_range_from_dkg_compatibility() {
    let version = "0.11.1-fedi17+fedi"
        .parse::<FedimintdVersion>()
        .expect("valid release version");

    assert_eq!(
        version.core(),
        FedimintdVersionCore {
            major: 0,
            minor: 11,
            patch: 1,
        }
    );
    assert_eq!(version.core().to_string(), "0.11.1");
    assert_eq!(version.dkg_version().to_string(), "0.11+fedi");
    assert!(version.dkg_version().is_fedi());
    assert_eq!(
        version.dkg_version(),
        "0.11.2-rc.1+fedi"
            .parse::<FedimintdVersion>()
            .expect("valid patch-skewed version")
            .dkg_version()
    );
    assert_ne!(
        version.dkg_version(),
        "0.11.2"
            .parse::<FedimintdVersion>()
            .expect("valid upstream version")
            .dkg_version()
    );
    assert_ne!(
        version.dkg_version(),
        "0.11.2+acme"
            .parse::<FedimintdVersion>()
            .expect("valid other-vendor version")
            .dkg_version()
    );
    assert!(
        serde_json::from_value::<FedimintdVersionCore>(serde_json::json!({
            "major": 0,
            "minor": 11,
            "patch": 1,
            "build": "fedi17"
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<FedimintdDkgVersion>(serde_json::json!({
            "major": 0,
            "minor": 11,
            "vendor": "not valid"
        }))
        .is_err()
    );
}

#[test]
fn meta_consensus_base_names_one_occurrence_not_its_content() {
    assert_eq!(
        MetaConsensusBase::from_consensus(None),
        MetaConsensusBase::Absent
    );
    assert_ne!(
        MetaConsensusBase::from_consensus(None),
        MetaConsensusBase::from_consensus(Some((0, &[])))
    );

    // The same (revision, bytes) occurrence recomputes the same base.
    let base = MetaConsensusBase::from_consensus(Some((7, b"abc")));
    assert_eq!(base, MetaConsensusBase::from_consensus(Some((7, b"abc"))));
    assert!(matches!(base, MetaConsensusBase::Sha256(_)));

    // Byte-identical content recurring under a fresh revision is a fresh
    // base: a reverted board cannot re-trigger old admissions or handlers.
    assert_ne!(base, MetaConsensusBase::from_consensus(Some((9, b"abc"))));
    // And the revision alone is not the base either.
    assert_ne!(base, MetaConsensusBase::from_consensus(Some((7, b"abz"))));

    let encoded = serde_json::to_value(base).expect("serialize base");
    assert_eq!(encoded["kind"], "sha256");
    assert_eq!(
        serde_json::from_value::<MetaConsensusBase>(encoded).expect("deserialize base"),
        base
    );
}
