use tempfile::TempDir;

use super::*;
use crate::seat_process::BitcoindConfig;

fn process(temp: &TempDir) -> SeatProcessConfig {
    SeatProcessConfig {
        data_root: temp.path().to_owned(),
        fedimintd: temp.path().join("fedimintd"),
        bitcoin_network: bitcoin::Network::Regtest,
        iroh_dns: "https://dns.iroh.link/pkarr".parse().unwrap(),
        bitcoin_backend: crate::seat_process::BitcoinBackend::Bitcoind(BitcoindConfig {
            url: "http://127.0.0.1:18443".to_owned(),
            username: "user".to_owned(),
            password: "pass".to_owned(),
        }),
    }
}

/// A restore no test here reaches: these exercise the choice, not the
/// documents. `recover` is covered where the documents are.
struct NoArchive;

struct NoHolderAuthorizations;

#[async_trait::async_trait]
impl HolderAuthorizationFetcher for NoHolderAuthorizations {
    async fn retained(
        &self,
        _identity: &RootMnemonic,
    ) -> anyhow::Result<Vec<fedi_decentralized_domain::HolderAuthorizationEnvelope>> {
        Ok(Vec::new())
    }

    async fn fetch(
        &self,
        _identity: &RootMnemonic,
    ) -> anyhow::Result<(Vec<FetchedHolderAuthorization>, u64)> {
        Ok((Vec::new(), u64::MAX))
    }
}

#[async_trait::async_trait]
impl crate::backup::BackupArchive for NoArchive {
    async fn recover(
        &self,
        _identity: &crate::identity::RootMnemonic,
    ) -> Result<crate::backup::RecoveredFleet, crate::backup::RecoverError> {
        Err(anyhow::anyhow!("no backup archive in this test").into())
    }
}

async fn onboarding(
    temp: &TempDir,
) -> (
    crate::admin::OperatorPhase,
    Arc<Onboarding>,
    Db,
    std::path::PathBuf,
) {
    let db = Db::open(temp.path()).await.unwrap();
    let onboarding = Onboarding::new(
        db.clone(),
        process(temp),
        Arc::new(NoArchive),
        Arc::new(NoHolderAuthorizations),
        true,
    );
    (
        crate::admin::OperatorPhase::onboarding(onboarding.clone()),
        onboarding,
        db,
        crate::admin::socket_path(temp.path()),
    )
}

/// The whole point of the phase: an operator chooses, and only then does this
/// host have a mnemonic. A daemon that has not been onboarded has no identity
/// at all — nothing mints one on its behalf.
#[tokio::test]
async fn onboarding_as_new_is_what_creates_the_identity() {
    let temp = TempDir::new().unwrap();
    let (phase, onboarding, db, socket) = onboarding(&temp).await;
    assert!(db.load_identity().await.unwrap().is_none());

    let _server = crate::admin::serve(&phase, &socket).unwrap();
    let socket = crate::admin::socket_path(temp.path());
    let reply = ask(&socket, &AdminRequest::OnboardAsNew { if_needed: false }).await;
    assert_eq!(reply.unwrap()["onboarded"], "new");

    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), onboarding.completed())
            .await
            .is_err(),
        "identity creation is only the first onboarding stage"
    );
    assert!(db.load_identity().await.unwrap().is_some());
}

/// Onboarding happens once, and the identity row is the enforcement: a second
/// attempt has nothing to do with a flag anything could set.
#[tokio::test]
async fn a_second_onboarding_cannot_replace_the_first() {
    let temp = TempDir::new().unwrap();
    let (phase, _onboarding, db, socket) = onboarding(&temp).await;
    let first = crate::identity::RootMnemonic::generate().unwrap();
    db.install_identity(&first).await.unwrap();

    let server = crate::admin::serve(&phase, &socket).unwrap();
    let socket = crate::admin::socket_path(temp.path());
    let refused = ask(&socket, &AdminRequest::OnboardAsNew { if_needed: false })
        .await
        .unwrap_err();
    assert!(
        refused.message.contains("already been onboarded"),
        "{refused:?}"
    );
    assert_eq!(
        db.load_identity().await.unwrap().unwrap().phrase(),
        first.phrase()
    );
    server.abort();
}

/// Everything else needs a fleet, and there is no fleet yet. The refusal is
/// asserted by its discriminant: the browser setup wizard opens on exactly
/// this refusal, and a consumer that read the sentence instead would break the
/// day the sentence was reworded.
#[tokio::test]
async fn an_un_onboarded_daemon_answers_only_the_onboarding_verbs() {
    let temp = TempDir::new().unwrap();
    let (phase, _onboarding, _db, socket) = onboarding(&temp).await;
    let server = crate::admin::serve(&phase, &socket).unwrap();
    let socket = crate::admin::socket_path(temp.path());

    for request in [AdminRequest::ListSeats, AdminRequest::ShowMnemonic] {
        let refused = ask(&socket, &request).await.unwrap_err();
        assert_eq!(
            refused.kind,
            crate::admin::AdminErrorKind::NotOnboarded,
            "{refused:?}"
        );
    }
    server.abort();
}

/// The one precondition only an operator can answer is refused where the
/// operator can see it.
#[tokio::test]
async fn restoring_needs_an_acknowledgement() {
    let temp = TempDir::new().unwrap();
    let (phase, _onboarding, db, socket) = onboarding(&temp).await;
    let phrase = crate::identity::RootMnemonic::generate().unwrap();
    let server = crate::admin::serve(&phase, &socket).unwrap();
    let socket = crate::admin::socket_path(temp.path());

    let unacknowledged = ask(
        &socket,
        &AdminRequest::OnboardFromBackup {
            mnemonic: phrase.phrase().to_owned(),
            acknowledge_original_host_is_gone: false,
        },
    )
    .await
    .unwrap_err();
    assert_eq!(
        unacknowledged.kind,
        crate::admin::AdminErrorKind::RestoreNotAcknowledged
    );
    assert!(
        unacknowledged.message.contains("permanently offline"),
        "{unacknowledged:?}"
    );

    assert!(db.load_identity().await.unwrap().is_none());
    server.abort();
}

/// A phrase that is not a phrase never becomes an identity, and never reaches
/// the error the operator reads.
#[tokio::test]
async fn a_phrase_that_is_not_a_phrase_is_refused_without_being_echoed() {
    let temp = TempDir::new().unwrap();
    let (phase, _onboarding, db, socket) = onboarding(&temp).await;
    let server = crate::admin::serve(&phase, &socket).unwrap();
    let socket = crate::admin::socket_path(temp.path());

    let refused = ask(
        &socket,
        &AdminRequest::OnboardFromBackup {
            mnemonic: "hunter2 hunter2 hunter2".to_owned(),
            acknowledge_original_host_is_gone: true,
        },
    )
    .await
    .unwrap_err();
    assert_eq!(refused.kind, crate::admin::AdminErrorKind::InvalidMnemonic);
    assert!(
        !refused.message.contains("hunter2"),
        "the phrase must not be echoed"
    );
    assert!(db.load_identity().await.unwrap().is_none());
    server.abort();
}

/// The documents a restore installs are the ones this module hands to
/// [`crate::restore`]; this pins that the phrase it returns is the phrase
/// those documents belong to, which is what the fleet is opened against.
#[tokio::test]
async fn a_restore_returns_the_identity_its_documents_belong_to() {
    let temp = TempDir::new().unwrap();
    let identity = crate::identity::RootMnemonic::generate().unwrap();
    let db = Db::open(temp.path()).await.unwrap();

    // Installing straight from a recovered fleet, as the socket path does
    // once the relay has answered.
    let recovered = crate::backup::RecoveredFleet {
        seats: Vec::new(),
        archives: std::collections::HashMap::new(),
        format_version: 1,
    };
    crate::restore::install(&db, &process(&temp), &identity, &recovered)
        .await
        .unwrap();
    assert_eq!(
        db.load_identity().await.unwrap().unwrap().phrase(),
        identity.phrase()
    );
    // And a second onboarding of any kind is now impossible.
    assert!(matches!(
        crate::restore::install(&db, &process(&temp), &identity, &recovered).await,
        Err(crate::restore::RestoreError::AlreadyOnboarded)
    ));
}

/// The completion watch mirrors only this process; a daemon that onboarded in
/// an earlier life learns it from the database, or startup would wait forever
/// on a wizard that never runs again.
#[tokio::test]
async fn completion_is_observed_across_a_restart() {
    let temp = TempDir::new().unwrap();
    {
        let (phase, _onboarding, db, _socket) = onboarding(&temp).await;
        db.install_identity(&crate::identity::RootMnemonic::generate().unwrap())
            .await
            .unwrap();
        db.merge_holder_authorization_events(&[(vec![1; 32], 1, "{}".to_owned())], 1)
            .await
            .unwrap();
        db.configure_initial_offer(None, 1).await.unwrap();
        drop(phase);
    }

    // A fresh phase over the same database: the restarted daemon.
    let (_phase, restarted, _db, _socket) = onboarding(&temp).await;
    tokio::time::timeout(std::time::Duration::from_secs(5), restarted.completed())
        .await
        .expect("a completed onboarding resolves immediately after restart")
        .unwrap();
}

async fn ask(
    socket: &std::path::Path,
    request: &AdminRequest,
) -> Result<serde_json::Value, crate::admin::AdminError> {
    for _ in 0..200 {
        match crate::admin::request(socket, request).await {
            Ok(response) => return response,
            // The server binds a moment after the task is spawned.
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(10)).await,
        }
    }
    panic!("the onboarding socket never answered");
}
