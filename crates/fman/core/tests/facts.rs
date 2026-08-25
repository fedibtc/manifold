use super::*;
#[test]
fn dkg_code_sets_are_validated_and_order_insensitive() {
    let code = |s: &str| GuardianCode(s.to_owned());
    let own = code("b");
    let size = FederationSize(3);

    let submitted = DkgCodeSet::validate(&[code("c"), own.clone(), code("a")], size, &own).unwrap();
    let reordered = DkgCodeSet::validate(&[code("a"), code("c"), own.clone()], size, &own).unwrap();
    assert_eq!(submitted, reordered);
    assert!(matches!(
        DkgCodeSet::validate(std::slice::from_ref(&own), size, &own),
        Err(DkgCodeSetError::WrongCount { .. })
    ));
    assert!(matches!(
        DkgCodeSet::validate(&[code("a"), code("a"), own.clone()], size, &own),
        Err(DkgCodeSetError::DuplicateCode)
    ));
    assert!(matches!(
        DkgCodeSet::validate(&[code("a"), code("c"), code("d")], size, &own),
        Err(DkgCodeSetError::OwnCodeMissing)
    ));
}
