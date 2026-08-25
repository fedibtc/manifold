use super::{FedimintdVersion, MetaConsensusBase};

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
