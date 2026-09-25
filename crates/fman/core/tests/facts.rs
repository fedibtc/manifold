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

#[test]
fn admission_ports_stop_before_the_default_linux_ephemeral_range() {
    let first = PortBase::new(30_000).unwrap();

    assert_eq!(
        SeatNo(691).admission_port_base(first).unwrap().get(),
        32_764
    );
    assert_eq!(
        SeatNo(691).admission_port_base(first).unwrap().last(),
        32_767
    );
    assert!(SeatNo(692).admission_port_base(first).is_none());
    assert!(
        SeatNo(0)
            .admission_port_base(PortBase::new(65_532).unwrap())
            .is_none(),
        "the highest valid u16 block is rejected without overflowing"
    );
    assert_eq!(
        SeatNo(692).port_base(first).unwrap().get(),
        LINUX_DEFAULT_EPHEMERAL_PORT_START,
        "durable seats retain their pre-upgrade mapping"
    );
}
