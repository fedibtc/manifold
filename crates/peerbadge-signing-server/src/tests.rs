use super::*;
use fedi_decentralized_manifold_environment::ManifoldEnvironment;
use nostr_sdk::Keys;
use peerbadge_protocol::{HolderContext, PendingIssuance};

fn fixture(max_level: u8, rate: u32) -> (SigningServer, Keys, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap();
    let signer = Keys::generate();
    let profile = ManifoldEnvironment::Development.profile().unwrap();
    let issuer = IssuerMaterial::load(&profile, None).unwrap();
    let server = SigningServer::new(
        issuer,
        vec![Signer {
            pubkey: signer.public_key().to_hex(),
            max_level,
            max_sessions_per_hour: rate,
        }],
        &directory.path().join("audit.jsonl"),
    )
    .unwrap();
    (server, signer, directory)
}

fn open_request(state: &mut State, signer: &Keys, level: u8, now: u64) -> OpenSessionRequest {
    let challenge = state.challenge(now).unwrap();
    let nonce = challenge.nonce.as_slice().try_into().unwrap();
    let digest = challenge_digest(&nonce, level, &state.issuer_id);
    OpenSessionRequest {
        signer_pubkey: signer.public_key().to_hex(),
        level,
        nonce: challenge.nonce,
        signature: signer
            .sign_schnorr(&Message::from_digest(digest))
            .as_ref()
            .to_vec(),
    }
}

fn redemption(state: &State, session: &OpenSessionResponse) -> RedeemSessionRequest {
    let holder = HolderContext::generate();
    let metadata = &state.issuer.authority.issuer;
    let (request, _) = PendingIssuance::create_request(
        &metadata.issuance_key,
        metadata.issuer_id_pubkey.clone(),
        session.info.clone(),
        serde_json::json!(holder.public_key().to_hex()),
    )
    .unwrap();
    RedeemSessionRequest {
        session_id: session.session_id.clone(),
        request: serde_json::to_string(&request).unwrap(),
    }
}

#[test]
fn canonical_authorities_and_production_requires_explicit_keys() {
    for environment in [
        ManifoldEnvironment::Development,
        ManifoldEnvironment::Staging,
    ] {
        let profile = environment.profile().unwrap();
        let material = IssuerMaterial::load(&profile, None).unwrap();
        assert_eq!(
            material.authority_json,
            profile.pinned_issuer_authorities()[0]
        );
        assert_eq!(
            material.keys.public_key(),
            profile.peer_badge_issuer_identities()[0]
        );
    }
    assert!(
        IssuerMaterial::load(&ManifoldEnvironment::Production.profile().unwrap(), None).is_err()
    );
}

#[test]
fn explicit_authority_is_identical_for_publication_and_session_offers() {
    let directory = tempfile::tempdir().unwrap();
    let profile = ManifoldEnvironment::Development.profile().unwrap();
    let mut secret: peerbadge_protocol::IssuerSecretKeys =
        serde_json::from_str(profile.test_issuer_secret_keys().unwrap()).unwrap();
    // Reuse the public test RSA key with a different test identity so the
    // authority cannot be satisfied by the environment's pinned document.
    secret.issuer_id_secret_key = format!("{:064x}", 3);
    let path = directory.path().join("issuer.json");
    std::fs::write(&path, serde_json::to_vec(&secret).unwrap()).unwrap();
    let published = IssuerMaterial::load(&profile, Some(&path)).unwrap();
    let served = IssuerMaterial::load(&profile, Some(&path)).unwrap();
    assert_eq!(published.authority_json, served.authority_json);
    served.authority.verify().unwrap();
    let signer = Keys::generate();
    let server = SigningServer::new(
        served,
        vec![Signer {
            pubkey: signer.public_key().to_hex(),
            max_level: 9,
            max_sessions_per_hour: 1,
        }],
        &directory.path().join("audit.jsonl"),
    )
    .unwrap();
    let mut state = server.state.lock().unwrap();
    let request = open_request(&mut state, &signer, 9, 100);
    assert_eq!(
        state.open(request, 100).unwrap().issuer_authority,
        published.authority_json
    );
}

#[test]
fn challenge_expiry_single_use_signature_and_issuer_binding() {
    let (server, signer, _) = fixture(9, 100);
    let mut state = server.state.lock().unwrap();
    let request = open_request(&mut state, &signer, 9, 100);
    assert_eq!(
        state.challenges[&<[u8; 32]>::try_from(request.nonce.as_slice()).unwrap()],
        160
    );
    let replay = request.clone();
    assert!(state.open(request, 159).is_ok());
    assert!(matches!(
        state.open(replay, 159),
        Err(SigningError::BadChallenge)
    ));
    let request = open_request(&mut state, &signer, 9, 100);
    assert!(matches!(
        state.open(request, 160),
        Err(SigningError::BadChallenge)
    ));
    let mut request = open_request(&mut state, &signer, 9, 200);
    request.signature[0] ^= 1;
    let replay = request.clone();
    assert!(matches!(
        state.open(request, 200),
        Err(SigningError::Unauthorized)
    ));
    assert!(matches!(
        state.open(replay, 200),
        Err(SigningError::BadChallenge)
    ));
    let mut request = open_request(&mut state, &signer, 9, 200);
    let digest = challenge_digest(&request.nonce.as_slice().try_into().unwrap(), 9, &[42; 32]);
    request.signature = signer
        .sign_schnorr(&Message::from_digest(digest))
        .as_ref()
        .to_vec();
    assert!(matches!(
        state.open(request, 200),
        Err(SigningError::Unauthorized)
    ));
    let stranger = Keys::generate();
    let request = open_request(&mut state, &stranger, 9, 200);
    assert!(matches!(
        state.open(request, 200),
        Err(SigningError::Unauthorized)
    ));
}

#[test]
fn schema_level_caps_sliding_rate_limit_and_capacity() {
    let (server, signer, _) = fixture(5, 1);
    let mut state = server.state.lock().unwrap();
    for level in [0, 6, 10, 255] {
        let request = open_request(&mut state, &signer, level, 100);
        assert!(matches!(
            state.open(request, 100),
            Err(SigningError::LevelNotAllowed { max: 5 })
        ));
    }
    let request = open_request(&mut state, &signer, 5, 100);
    state.open(request, 100).unwrap();
    let request = open_request(&mut state, &signer, 5, 101);
    assert!(matches!(
        state.open(request, 101),
        Err(SigningError::RateLimited)
    ));
    state.sweep(3699).unwrap();
    let request = open_request(&mut state, &signer, 5, 3699);
    assert!(matches!(
        state.open(request, 3699),
        Err(SigningError::RateLimited)
    ));
    state.sweep(3700).unwrap();
    let request = open_request(&mut state, &signer, 5, 3700);
    state.open(request, 3700).unwrap();
    state.challenges.clear();
    for n in 0..MAX_ENTRIES {
        let mut nonce = [0; 32];
        nonce[..8].copy_from_slice(&(n as u64).to_le_bytes());
        state.challenges.insert(nonce, 4000);
    }
    assert!(matches!(
        state.challenge(3700),
        Err(SigningError::RateLimited)
    ));
    state.sweep(4000).unwrap();
    assert!(state.challenge(4000).is_ok());
}

#[test]
fn redeem_once_invalid_requests_do_not_consume_and_audit_is_private() {
    let (server, signer, directory) = fixture(9, 10);
    let mut state = server.state.lock().unwrap();
    let request = open_request(&mut state, &signer, 9, 100);
    let opened = state.open(request, 100).unwrap();
    assert_eq!(opened.expires_at, 700);
    assert_eq!(opened.session_id.len(), 64);
    assert_eq!(
        opened.info,
        serde_json::json!({"schema": "fedi-trust-score-v1.0", "trust_level": 9})
    );
    let bad = RedeemSessionRequest {
        session_id: opened.session_id.clone(),
        request: "PRIVATE-MALFORMED-REQUEST".to_owned(),
    };
    let error = state.redeem(bad, 100).unwrap_err();
    assert!(matches!(error, SigningError::InvalidRequest(_)));
    assert!(!format!("{error:?}").contains("PRIVATE"));
    let request = redemption(&state, &opened);
    let raw_request = request.request.clone();
    state.redeem(request.clone(), 101).unwrap();
    assert!(matches!(
        state.redeem(request, 101),
        Err(SigningError::SessionAlreadyRedeemed)
    ));
    assert!(matches!(
        state.sessions[&opened.session_id].state,
        SessionState::Redeemed
    ));
    state.sweep(700).unwrap();
    assert!(matches!(
        state.sessions[&opened.session_id].state,
        SessionState::Redeemed
    ));
    let audit = std::fs::read_to_string(directory.path().join("audit.jsonl")).unwrap();
    assert!(!audit.contains("PRIVATE"));
    assert!(!audit.contains(&raw_request));
    assert!(!audit.contains("blinded_message"));
    let lines: Vec<serde_json::Value> = audit
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["event"], "session_opened");
    assert_eq!(lines[1]["event"], "session_redeemed");
    assert_eq!(
        lines[1]["request_sha256"],
        hex::encode(Sha256::digest(raw_request.as_bytes()))
    );
    state.sweep(1300).unwrap();
    assert!(state.sessions.is_empty());
}

#[test]
fn expiry_is_audited_once_and_unknown_sessions_are_distinct() {
    let (server, signer, directory) = fixture(9, 10);
    let mut state = server.state.lock().unwrap();
    let request = open_request(&mut state, &signer, 9, 100);
    let opened = state.open(request, 100).unwrap();
    state.sweep(700).unwrap();
    state.sweep(701).unwrap();
    assert!(matches!(
        state.sessions[&opened.session_id].state,
        SessionState::Expired
    ));
    assert!(matches!(
        state.redeem(
            RedeemSessionRequest {
                session_id: opened.session_id,
                request: String::new()
            },
            701
        ),
        Err(SigningError::SessionExpired)
    ));
    assert!(matches!(
        state.redeem(
            RedeemSessionRequest {
                session_id: "missing".to_owned(),
                request: String::new()
            },
            701
        ),
        Err(SigningError::SessionNotFound)
    ));
    let audit = std::fs::read_to_string(directory.path().join("audit.jsonl")).unwrap();
    assert_eq!(audit.matches("session_expired").count(), 1);
}

#[test]
fn malformed_allowlists_fail_at_startup() {
    let profile = ManifoldEnvironment::Development.profile().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let signer = Signer {
        pubkey: Keys::generate().public_key().to_hex(),
        max_level: 9,
        max_sessions_per_hour: 1,
    };
    for signers in [
        vec![signer.clone(), signer.clone()],
        vec![Signer {
            max_level: 10,
            ..signer.clone()
        }],
        vec![Signer {
            max_sessions_per_hour: 0,
            ..signer.clone()
        }],
        vec![Signer {
            pubkey: "not-a-pubkey".to_owned(),
            ..signer.clone()
        }],
    ] {
        assert!(
            SigningServer::new(
                IssuerMaterial::load(&profile, None).unwrap(),
                signers,
                &dir.path().join("audit.jsonl")
            )
            .is_err()
        );
    }
}

#[test]
fn session_capacity_and_rejection_audit_are_bounded() {
    let (server, signer, directory) = fixture(9, 10);
    let mut state = server.state.lock().expect("state lock");
    for index in 0..MAX_ENTRIES {
        state.sessions.insert(
            format!("{index:064x}"),
            Session {
                signer: signer.public_key().to_hex(),
                level: 9,
                expires_at: 700,
                state: SessionState::Redeemed,
            },
        );
    }
    let request = open_request(&mut state, &signer, 9, 100);
    assert!(matches!(
        state.open(request, 100),
        Err(SigningError::RateLimited)
    ));
    assert_eq!(state.sessions.len(), MAX_ENTRIES);
    let request = OpenSessionRequest {
        signer_pubkey: "PRIVATE-UNTRUSTED-TEXT".to_owned(),
        level: 9,
        nonce: vec![1; 31],
        signature: vec![2; 63],
    };
    assert!(matches!(
        state.open(request, 100),
        Err(SigningError::BadChallenge)
    ));
    let audit = std::fs::read_to_string(directory.path().join("audit.jsonl")).unwrap();
    assert!(!audit.contains("PRIVATE"));
    let lines: Vec<serde_json::Value> = audit
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["event"], "open_rejected");
    assert_eq!(lines[0]["reason"], "rate_limited");
    assert_eq!(lines[1]["reason"], "bad_challenge");
    assert!(lines[1]["signer_pubkey"].is_null());
    state.sweep(1300).unwrap();
    let request = open_request(&mut state, &signer, 9, 1300);
    assert!(state.open(request, 1300).is_ok());
}

#[tokio::test]
async fn status_distinguishes_unknown_and_expired_capabilities() {
    let (server, signer, _directory) = fixture(9, 10);
    assert!(matches!(
        server
            .session_status(SessionStatusRequest {
                session_id: "unknown".to_owned()
            })
            .await,
        Err(SigningError::SessionNotFound)
    ));
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let session_id = {
        let mut state = server.state.lock().expect("state lock");
        let request = open_request(&mut state, &signer, 9, now - SESSION_TTL);
        state.open(request, now - SESSION_TTL).unwrap().session_id
    };
    assert_eq!(
        server
            .session_status(SessionStatusRequest { session_id })
            .await
            .unwrap()
            .state,
        SessionState::Expired
    );
}
