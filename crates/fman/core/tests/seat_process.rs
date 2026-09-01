use super::*;
use std::os::fd::AsRawFd as _;

#[tokio::test]
async fn protocol_frames_round_trip_over_socketpair() {
    let (left, right) = std::os::unix::net::UnixStream::pair().unwrap();
    left.set_nonblocking(true).unwrap();
    right.set_nonblocking(true).unwrap();
    let mut left = tokio::net::UnixStream::from_std(left).unwrap();
    let mut right = tokio::net::UnixStream::from_std(right).unwrap();
    let message = ParentMessage::RunDkg {
        our_index: 0,
        codes: vec!["fed11test".to_owned()],
        iroh_api_sk: [1; 32],
        iroh_p2p_sk: [2; 32],
        tls_key: None,
        api_auth: "secret".to_owned(),
        network: "regtest".to_owned(),
    };
    write_frame(&mut left, &message).await.unwrap();
    let decoded: ParentMessage = read_frame(&mut right).await.unwrap();
    let ParentMessage::RunDkg {
        our_index, codes, ..
    } = decoded;
    assert_eq!(our_index, 0);
    assert_eq!(codes, ["fed11test"]);
}

#[test]
fn startup_frames_fit_socketpair_without_a_reader() {
    let hello = ChildMessage::Hello {
        proto: PROTOCOL_VERSION,
        code_version: "v0.11.1-fedi18".to_owned(),
        state: ChildState::AlreadyConfigured {
            invite_code: "fed1-test".repeat(32),
        },
    };
    let consensus = ChildMessage::ConsensusStarted {};
    let mut bytes = Vec::new();
    ciborium::into_writer(&hello, &mut bytes).unwrap();
    ciborium::into_writer(&consensus, &mut bytes).unwrap();

    let (writer, _reader) = std::os::unix::net::UnixStream::pair().unwrap();
    let send_buffer: libc::c_int = unsafe {
        let mut value = 0;
        let mut len = std::mem::size_of_val(&value) as libc::socklen_t;
        assert_eq!(
            libc::getsockopt(
                writer.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_SNDBUF,
                (&mut value as *mut libc::c_int).cast(),
                &mut len
            ),
            0
        );
        value
    };
    assert!(bytes.len() + 8 < send_buffer as usize);
}

#[tokio::test]
async fn child_receives_configured_iroh_dns_relay() {
    let temp = tempfile::tempdir().unwrap();
    let args_path = temp.path().join("args");
    let env_path = temp.path().join("iroh-dns");
    let fedimintd = fake::write_fake_fedimintd(
        temp.path(),
        &format!(
            "printf '%s\\n' \"$@\" > '{}'; printf '%s' \"$FM_IROH_DNS\" > '{}'",
            args_path.display(),
            env_path.display()
        ),
    )
    .await;
    let relay = "https://pkarr.example.test/iroh";
    let config = SeatProcessConfig {
        data_root: temp.path().to_owned(),
        fedimintd,
        bitcoin_network: bitcoin::Network::Regtest,
        bitcoin_backend: BitcoinBackend::Esplora("http://127.0.0.1:3000".parse().unwrap()),
        iroh_dns: relay.parse().unwrap(),
    };

    let mut child = SeatProcess::start(
        &config,
        SeatId::new("00".repeat(32)).unwrap(),
        SeatNo(0),
        SeatPorts::from_base(crate::facts::PortBase::new(31_000).unwrap()),
    )
    .await
    .unwrap();
    child.wait().await.unwrap();

    assert_eq!(tokio::fs::read_to_string(env_path).await.unwrap(), relay);
    let args = tokio::fs::read_to_string(args_path).await.unwrap();
    assert!(args.lines().any(|arg| arg == "--enable-iroh"));
}

/// The iroh-carrying ports (p2p, api) must bind all interfaces — fedimintd
/// places its iroh UDP sockets at those addresses, and loopback there forces
/// relay-only peering — while ui and metrics stay private to the host.
/// This pins the production arm only: under FMAN_E2E_LOCAL_IROH the spawn
/// keeps all four ports on loopback (the harness's port-derived keys are
/// publicly derivable), and this test assumes that variable is absent from
/// the test process environment.
#[tokio::test]
async fn iroh_carrying_ports_bind_all_interfaces_others_loopback() {
    let temp = tempfile::tempdir().unwrap();
    let args_path = temp.path().join("args");
    let fedimintd = fake::write_fake_fedimintd(
        temp.path(),
        &format!("printf '%s\\n' \"$@\" > '{}'", args_path.display()),
    )
    .await;
    let config = SeatProcessConfig {
        data_root: temp.path().to_owned(),
        fedimintd,
        bitcoin_network: bitcoin::Network::Regtest,
        bitcoin_backend: BitcoinBackend::Esplora("http://127.0.0.1:3000".parse().unwrap()),
        iroh_dns: "https://pkarr.example.test/iroh".parse().unwrap(),
    };

    let mut child = SeatProcess::start(
        &config,
        SeatId::new("00".repeat(32)).unwrap(),
        SeatNo(0),
        SeatPorts::from_base(crate::facts::PortBase::new(31_000).unwrap()),
    )
    .await
    .unwrap();
    child.wait().await.unwrap();

    let args_raw = tokio::fs::read_to_string(args_path).await.unwrap();
    let args: Vec<&str> = args_raw.lines().collect();
    let bind = |flag: &str| {
        let position = args.iter().position(|arg| *arg == flag).unwrap();
        args[position + 1]
    };
    assert_eq!(bind("--bind-p2p"), "0.0.0.0:31000");
    assert_eq!(bind("--bind-api"), "0.0.0.0:31001");
    assert_eq!(bind("--bind-ui"), "127.0.0.1:31002");
    assert_eq!(bind("--bind-metrics"), "127.0.0.1:31003");
}
