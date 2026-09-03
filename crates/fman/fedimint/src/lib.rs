//! In-process Fedimint client wallet for FI-to-FMan OOB ecash payments.
//!
//! One [`Wallet`] holds a payment client and one guardian-fee client per
//! guarded seat/federation scope under one mnemonic-derived wallet secret. A
//! single RocksDB is partitioned by monotonically allocated, prefix-free
//! consensus-encoded scope prefixes; prefix zero holds the allocator and its
//! prefix-to-scope map.
//!
//! Which payment federations *should* be joined is not this crate's concern:
//! callers supply authenticated policy — [`setup_payment_policy`] takes a
//! watch of the admitted common set and maintains joins against it — and
//! this crate is the mechanism that joins, receives, spends, and reports
//! balances.
//!
//! This crate is FMan-only: the payee side of the key-locked payment
//! protocol (`payee`, the real implementation of `fman-core`'s
//! [`EcashWallet`](fman_core::wallet::EcashWallet) hole, with
//! `guardian_fee` filling its fee-collection hole, and
//! `setup_payment_policy`) on top of this root's client wallet. Everything
//! the FI (payer) side must compute identically — the per-generation
//! cryptography, denomination selection, and the refund preparation the
//! payee re-runs as its validation oracle — lives in the shared
//! `locked-payments` crate, not here; the payer itself
//! is `fi-cli`'s concern and never touches this crate.
//!
//! Bearer-token hygiene: raw OOB
//! tokens are never logged and never persisted by this crate; a received
//! token exists in memory only for the duration of [`Wallet::receive`].

#[cfg(test)]
mod authority_surface_tests;
mod claim_worker;
mod drain_status;
mod guardian_fee;
mod payee;
#[cfg(test)]
mod payout_contract_tests;
mod payout_job;
mod payout_job_status;
mod payout_native;
mod payout_observer;
mod payout_operation_id;
mod payout_store;
mod payout_worker;
pub mod setup_payment_policy;
mod wallet_drain;

pub(crate) use payout_native::{await_payout, payout_for_request, payout_status, start_payout};

pub use fman_core::db::WalletOrigin;
pub use fman_core::guardian_fee::GuardianFeeAccountKey;
pub use fman_core::wallet::WalletSecret;

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::PathBuf;
use std::str::FromStr as _;
use std::sync::Arc;
use zeroize::Zeroizing;

use anyhow::Context as _;
use bitcoin_hashes::{Hash as _, HashEngine as _, sha256, sha256t};
use fedimint_client::db::{ClientInitStateKey, ClientModuleRecovery};
use fedimint_client::{Client, ClientHandle, ClientHandleArc, RootSecret};
use fedimint_client_module::module::ClientModule as _;
use fedimint_connectors::ConnectorRegistry;
use fedimint_core::Amount;
use fedimint_core::config::FederationId;
use fedimint_core::core::{ModuleInstanceId, OperationId};
use fedimint_core::db::{Database, IDatabaseTransactionOpsCore, IDatabaseTransactionOpsCoreTyped};
use fedimint_core::encoding::{Decodable, Encodable};
use fedimint_core::invite_code::InviteCode;
use fedimint_core::module::CommonModuleInit as _;
use fedimint_core::module::registry::ModuleDecoderRegistry;
use fedimint_derive_secret::DerivableSecret;
use fedimint_ln_client::{LightningClientInit, LightningClientModule};
use fedimint_lnv2_client::LightningClientInit as LightningV2ClientInit;
use fedimint_meta_client::MetaClientInit;
use fedimint_mint_client::{
    MintClientInit, MintClientModule, OOBNotes, ReissueExternalNotesError,
    ReissueExternalNotesState,
};
use fedimint_mintv2_client::MintClientInit as MintV2ClientInit;
use fedimint_mintv2_client::{
    ECash as MintV2Ecash, FinalReceiveOperationState as MintV2ReceiveState,
    MintClientModule as MintV2ClientModule,
};
use fedimint_mintv2_common::Denomination as MintV2Denomination;
use futures::StreamExt as _;
use lightning_invoice::Bolt11Invoice;
use lnurl::LnUrlResponse;
use lnurl::lightning_address::LightningAddress;
use lnurl::lnurl::LnUrl;
use locked_payments::{locked_payment, locked_payment_v2, mint_v2_module, refund};
use stability_pool_client::StabilityPoolClientInit;
use tokio::sync::{Mutex, RwLock};
use tracing::warn;

const WALLET_SECRET_SALT: &[u8] = b"fleet-manager-wallet-v0";
const WALLET_LOCK_FILE: &str = ".wallet.lock";
/// Maximum decoded body retained from either LNURL-pay response.
///
/// LNURL-pay responses contain a callback URL, human-readable metadata, amount
/// bounds, and (on the second request) one BOLT 11 invoice. 64 KiB leaves ample
/// room for those fields while keeping a remote endpoint from growing one payout
/// request's JSON allocation without bound.
const MAX_LNURL_RESPONSE_BYTES: usize = 64 * 1024;
const JOIN_ATTEMPT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const MINIMUM_V1_GATEWAY_TTL: std::time::Duration = std::time::Duration::from_secs(30);

/// Wallet failures, shaped for the protocol errors the FMan must answer
/// with (`PaymentFederationNotAccepted`, `InvalidPayment`, ...); the caller
/// owns that mapping.
#[derive(Debug, thiserror::Error)]
pub enum WalletError {
    /// The federation is not joined by this wallet.
    #[error("federation {0} is not joined")]
    UnknownFederation(FederationId),

    /// The submitted string is not a parsable OOB ecash token.
    #[error("invalid ecash token")]
    InvalidToken(#[source] anyhow::Error),

    /// The token was minted by a different federation than the one the
    /// payer selected.
    #[error("token does not belong to federation {selected}")]
    WrongFederation {
        /// Federation the payer selected for the payment.
        selected: FederationId,
    },

    /// The federation refused reissuance (double-spent/invalid notes) or
    /// could not be reached to complete it.
    #[error("ecash receive failed: {0}")]
    ReceiveFailed(String),

    /// The wallet does not hold enough ecash in this federation.
    #[error("insufficient balance in federation")]
    InsufficientBalance,

    /// A private invite would be logged by the pinned Fedimint client while
    /// downloading its configuration.
    #[error("payment federation invite must not contain an API secret")]
    PrivateInviteUnsupported,

    /// The soft attempt budget elapsed.
    #[error("wallet federation {federation_id} join timed out")]
    JoinTimedOut {
        /// Federation whose attempt exceeded the reporting budget.
        federation_id: FederationId,
    },

    /// Any other client failure (open, join, spend, balance).
    #[error(transparent)]
    Client(#[from] anyhow::Error),

    /// Key-locked payment evidence or refund outputs are invalid.
    #[error("invalid key-locked payment: {0}")]
    InvalidLockedPayment(#[from] locked_payment::LockedPaymentError),

    /// Mint-v2 key-locked payment evidence or refund outputs are invalid.
    #[error("invalid mint-v2 key-locked payment: {0}")]
    InvalidLockedPaymentV2(#[from] locked_payment_v2::LockedPaymentV2Error),
}

impl From<refund::RefundError> for WalletError {
    fn from(error: refund::RefundError) -> Self {
        match error {
            refund::RefundError::V1(error) => Self::InvalidLockedPayment(error),
            refund::RefundError::V2(error) => Self::InvalidLockedPaymentV2(error),
            refund::RefundError::Client(error) => Self::Client(error),
        }
    }
}

/// Wallet-owned root bytes that zeroize on drop and cannot be formatted.
struct WalletSecretBytes(Zeroizing<[u8; 64]>);

impl WalletSecretBytes {
    /// Borrow the root bytes for deterministic payment-secret derivation.
    fn expose_for_derivation(&self) -> &[u8; 64] {
        &self.0
    }
}

#[derive(Clone, Debug, Decodable, Encodable, Eq, Hash, Ord, PartialEq, PartialOrd)]
enum ClientScope {
    Payment(FederationId),
    Guardian {
        federation_id: FederationId,
        seat_id: String,
    },
}

impl ClientScope {
    fn federation_id(&self) -> FederationId {
        match self {
            Self::Payment(id)
            | Self::Guardian {
                federation_id: id, ..
            } => *id,
        }
    }
}

/// One payment client and, for every guarded seat, one guardian-fee client per
/// federation under one data directory and mnemonic-derived wallet secret.
pub struct Wallet {
    database: Database,
    /// Federation id to its never-reused prefix-free consensus database prefix.
    prefixes: Arc<RwLock<BTreeMap<ClientScope, u64>>>,
    /// Serializes global prefix allocation; federation joins remain independently locked.
    prefix_allocation: Mutex<()>,
    root_secret: DerivableSecret,
    /// Root of the guardian-fee clients, from its own identity label. `None`
    /// on a wallet that never guards (the FI side opens one of those).
    guardian_root_secret: Option<DerivableSecret>,
    wallet_secret: WalletSecretBytes,
    /// Whether an uninitialized scope is proven never used (`Fresh`) or must
    /// scan mnemonic-derived keys because this identity came from restore.
    origin: WalletOrigin,
    federations: Arc<RwLock<BTreeMap<ClientScope, ClientHandleArc>>>,
    /// Serializes joins per federation while allowing unrelated federation
    /// joins to make progress independently.
    join_locks: Mutex<BTreeMap<ClientScope, Arc<Mutex<()>>>>,
    /// Serializes payout starts per wallet scope so v1 invoice replay checks
    /// cannot race another FMan payout start.
    payout_locks: Mutex<BTreeMap<ClientScope, Arc<Mutex<()>>>>,
    /// Serializes guardian-fee collections per seat-scoped wallet.
    collection_locks: Mutex<BTreeMap<ClientScope, Arc<Mutex<()>>>>,
    /// Process-lifetime fence for client opens. Once an open starts,
    /// cancellation or failure requires process restart before retry so a
    /// dependency task left behind cannot race a second database opener.
    open_attempted: Mutex<HashSet<ClientScope>>,
    /// Exclusive, non-symlink-following lock on the wallet root, held until
    /// the wallet drops. RocksDB's own lock does not fail a second opener, it
    /// blocks it, so this is what turns a concurrent open into an error.
    /// Declared last so ordinary field teardown drops all client handles
    /// before releasing root exclusivity.
    _data_dir_lock: Arc<std::fs::File>,
}

impl Wallet {
    /// Open the shared RocksDB under `data_dir`. Clients are opened lazily:
    /// guarded federations require the seat's committed account key.
    ///
    /// `secret` is the wallet root secret (the FMan derives it from its
    /// root identity material); per-federation secrets are derived from it
    /// by fedimint-client's standard double-derivation, so all federation
    /// wallets are recoverable from the FMan root alone.
    ///
    /// Acquires an exclusive, non-symlink-following lock under `data_dir` and
    /// holds it until the wallet drops. Opening a second wallet on the same
    /// root fails with the lock error as its source.
    pub async fn open(
        data_dir: PathBuf,
        secret: &WalletSecret,
        origin: WalletOrigin,
    ) -> anyhow::Result<Self> {
        Self::open_inner(data_dir, secret, None, origin).await
    }

    /// Open a wallet that also collects guardian fees.
    ///
    /// `guardian_secret` is a separate identity-derived root, not a variation
    /// on `secret`: a payment and a guardian client of the *same* federation
    /// open two databases under one root, so sharing one would give both the
    /// same module secret and the same sequential mint indices — colliding
    /// issuance and unrecoverable notes.
    pub async fn open_guarding(
        data_dir: PathBuf,
        secret: &WalletSecret,
        guardian_secret: &WalletSecret,
        origin: WalletOrigin,
    ) -> anyhow::Result<Self> {
        Self::open_inner(data_dir, secret, Some(guardian_secret), origin).await
    }

    async fn open_inner(
        data_dir: PathBuf,
        secret: &WalletSecret,
        guardian_secret: Option<&WalletSecret>,
        origin: WalletOrigin,
    ) -> anyhow::Result<Self> {
        let secret = &secret.0;
        let root_secret = DerivableSecret::new_root(secret, WALLET_SECRET_SALT);
        let guardian_root_secret = guardian_secret
            .map(|guardian| DerivableSecret::new_root(&guardian.0, WALLET_SECRET_SALT));
        tokio::fs::create_dir_all(&data_dir).await?;
        #[cfg(unix)]
        use std::os::unix::fs::OpenOptionsExt as _;
        let mut lock_options = std::fs::OpenOptions::new();
        lock_options.read(true).write(true).create(true);
        #[cfg(unix)]
        lock_options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
        let data_dir_lock = lock_options
            .open(data_dir.join(WALLET_LOCK_FILE))
            .context("open fedimint data-directory lock without following symlinks")?;
        data_dir_lock
            .try_lock()
            .context("lock fedimint data directory")?;
        let data_dir_lock = Arc::new(data_dir_lock);
        let database: Database = fedimint_rocksdb::RocksDb::build(data_dir.join("client.db"))
            .open()
            .await
            .context("open shared fedimint rocksdb")?
            .into();
        let prefixes = load_prefixes(&database).await?;
        Ok(Self {
            database,
            prefixes: Arc::new(RwLock::new(prefixes)),
            prefix_allocation: Mutex::new(()),
            _data_dir_lock: data_dir_lock,
            root_secret,
            guardian_root_secret,
            wallet_secret: WalletSecretBytes(Zeroizing::new(*secret)),
            origin,
            federations: Arc::new(RwLock::new(BTreeMap::new())),
            join_locks: Mutex::new(BTreeMap::new()),
            payout_locks: Mutex::new(BTreeMap::new()),
            collection_locks: Mutex::new(BTreeMap::new()),
            open_attempted: Mutex::new(HashSet::new()),
        })
    }

    /// Join with exactly one root client database per federation.  Prefixes
    /// are reserved before Fedimint gets the database and never reused, so a
    /// cancelled join cannot make a later federation inherit its state.
    pub async fn join(&self, invite_code: &InviteCode) -> Result<FederationId, WalletError> {
        self.join_with_timeout(
            invite_code,
            ClientScope::Payment(invite_code.federation_id()),
            JOIN_ATTEMPT_TIMEOUT,
            None,
        )
        .await
    }

    async fn join_with_timeout(
        &self,
        invite_code: &InviteCode,
        scope: ClientScope,
        timeout: std::time::Duration,
        guardian_key: Option<&GuardianFeeAccountKey>,
    ) -> Result<FederationId, WalletError> {
        ensure_public_invite(invite_code)?;
        self.join_inner(invite_code, scope, timeout, guardian_key)
            .await
    }

    async fn join_inner(
        &self,
        invite_code: &InviteCode,
        scope: ClientScope,
        preview_timeout: std::time::Duration,
        guardian_key: Option<&GuardianFeeAccountKey>,
    ) -> Result<FederationId, WalletError> {
        let federation_id = scope.federation_id();
        let _joining = self.join_exclusion(scope.clone()).await;
        if self.federations.read().await.contains_key(&scope) {
            return Ok(federation_id);
        }
        let known_prefix = { self.prefixes.read().await.get(&scope).copied() };
        let prefix = match known_prefix {
            Some(prefix) => prefix,
            None => {
                let _allocation = self.prefix_allocation.lock().await;
                let prefix = reserve_prefix(&self.database, &scope).await?;
                self.prefixes.write().await.insert(scope.clone(), prefix);
                prefix
            }
        };
        let client_root = self.client_root(&scope)?;
        let database = self.database.with_prefix(prefix.consensus_encode_to_vec());
        let initialized = Client::is_initialized(&database).await;
        let client = if initialized {
            client_open_once(&self.open_attempted, scope.clone(), async {
                self.open_initialized_scope(
                    &scope,
                    &database,
                    &client_root,
                    guardian_key,
                    "open joined federation",
                )
                .await
            })
            .await?
        } else {
            // Preview is network-only and precedes both the permanent open fence
            // and the initial client write, so a transient download failure can be
            // retried in-process without risking concurrent database writers.
            let preview = match tokio::time::timeout(preview_timeout, async {
                client_builder(guardian_key)
                    .await?
                    .preview(connectors().await?, invite_code)
                    .await
                    .context("download federation config from invite code")
            })
            .await
            {
                Ok(result) => result?,
                Err(_) => {
                    return Err(WalletError::JoinTimedOut {
                        federation_id: scope.federation_id(),
                    });
                }
            };
            validate_scope_config(&scope, preview.config())?;
            let recovery_scope = scope.clone();
            client_open_once(&self.open_attempted, scope.clone(), async move {
                let root = RootSecret::StandardDoubleDerive(client_root.clone());
                // Fresh onboarding plus the never-removed scope map proves an
                // absent federation-derived root was never used, so it may take
                // Fedimint's fast join path. A restored mnemonic has no such
                // proof and must scan. Recovery handles have no usable modules:
                // finish the scan, shut down the handle, then reopen the database.
                match self.origin {
                    WalletOrigin::Fresh => preview
                        .join(database, root)
                        .await
                        .context("join fresh federation"),
                    WalletOrigin::Restored => {
                        let recovering = preview
                            .recover(database.clone(), root, None)
                            .await
                            .context("recover federation")?;
                        self.finish_recovery(
                            &recovery_scope,
                            &database,
                            &client_root,
                            guardian_key,
                            recovering,
                            "recover federation",
                        )
                        .await
                    }
                }
            })
            .await?
        };
        validate_scope_config(&scope, &client.config().await)?;
        validate_scope_prefix(prefix, &scope, client.federation_id())?;
        self.federations
            .write()
            .await
            .insert(scope, Arc::new(client));
        Ok(federation_id)
    }

    /// Derive a mint-v1 locked quote issuance set and its escrow secrets
    /// from the wallet root and the quote's public randomness. Stateless
    /// and deterministic: repeating the call reproduces the same set.
    /// Payee-only: the shared refund preparation's mint-v1 fee-shape
    /// template uses a fixed seed instead of the wallet root.
    pub(crate) fn derive_locked_v1_quote(
        &self,
        quote_nonce: &[u8; 32],
        denominations: &[Amount],
    ) -> (
        Vec<locked_payment::IssuanceRequest>,
        Vec<locked_payment::NoteSecrets>,
    ) {
        locked_payment::derive_issuance_requests(
            self.wallet_secret.expose_for_derivation(),
            quote_nonce,
            denominations,
        )
    }

    /// Derive standard-recoverable mint-v2 quote outputs from this wallet's
    /// root. Role-neutral: the payee derives the real quote outputs with
    /// it, and the shared refund preparation runs the same derivation
    /// under the calling wallet's own root as a fee-shape template.
    pub(crate) async fn derive_locked_v2_quote(
        &self,
        federation_id: FederationId,
        mint_module: ModuleInstanceId,
        denominations: &[MintV2Denomination],
        tweaks: &[[u8; 16]],
    ) -> Result<Vec<locked_payment_v2::IssuanceRequest>, WalletError> {
        let client = self.client(federation_id).await?;
        let _ = mint_v2_module(&client, mint_module)?;
        Ok(locked_payment_v2::derive_standard_issuance_requests(
            &self.root_secret,
            federation_id,
            mint_module,
            denominations,
            tweaks,
        )?
        .0)
    }

    /// The client root a scope opens under. The guardian root is absent on a
    /// wallet opened by [`Wallet::open`], which has no guardian role at all.
    fn client_root(&self, scope: &ClientScope) -> Result<DerivableSecret, WalletError> {
        match scope {
            ClientScope::Payment(_) => Ok(self.root_secret.clone()),
            ClientScope::Guardian { seat_id, .. } => self
                .guardian_root_secret
                .as_ref()
                .context("wallet was opened without a guardian-fee root secret")
                .map(|root| guardian_scope_root(root, seat_id))
                .map_err(WalletError::Client),
        }
    }

    /// Open an initialized partition and finish a crash-resumed recovery before
    /// returning any handle to its caller.
    async fn open_initialized_scope(
        &self,
        scope: &ClientScope,
        database: &Database,
        client_root: &DerivableSecret,
        guardian_key: Option<&GuardianFeeAccountKey>,
        operation: &str,
    ) -> anyhow::Result<ClientHandle> {
        validate_stored_scope(database, scope).await?;
        let root = RootSecret::StandardDoubleDerive(client_root.clone());
        let client = client_builder(guardian_key)
            .await?
            .open(connectors().await?, database.clone(), root)
            .await
            .with_context(|| operation.to_owned())?;
        // A crashed fresh join can leave Pending(Fresh). Upstream open turns it
        // into Complete(Fresh), so classify only afterwards; recovery-mode
        // opens remain pending here until their module scans finish.
        let recovery_needs_ready_marker = recovery_needs_ready_marker(database).await?;
        if !client.has_pending_recoveries() {
            if recovery_needs_ready_marker {
                self.finish_completed_recovery(scope, database, &client)
                    .await?;
            }
            return Ok(client);
        }

        // Recovery initializes the database before its scans finish. A restart
        // reaches this ordinary-open path, which must not publish the handle
        // while its recovering modules are absent.
        self.finish_recovery(
            scope,
            database,
            client_root,
            guardian_key,
            client,
            "resume federation recovery",
        )
        .await
    }

    /// Confirm and durably record recovery readiness after upstream marked it
    /// complete before the recovered output state machines settled.
    async fn finish_completed_recovery(
        &self,
        scope: &ClientScope,
        database: &Database,
        client: &ClientHandle,
    ) -> anyhow::Result<()> {
        let config = client.config().await;
        validate_scope_config(scope, &config)?;
        let required_mints = required_mint_modules(&config)?;
        ensure_mint_recoveries_finished(database, &required_mints).await?;
        client
            .wait_for_all_active_state_machines()
            .await
            .context("settle output state machines from completed federation recovery")?;
        mark_recovery_ready(database).await
    }

    /// Wait for every required mint recovery, reopen its completed modules, and
    /// settle recovered output state machines before the client becomes usable.
    async fn finish_recovery(
        &self,
        scope: &ClientScope,
        database: &Database,
        client_root: &DerivableSecret,
        guardian_key: Option<&GuardianFeeAccountKey>,
        recovering: ClientHandle,
        operation: &str,
    ) -> anyhow::Result<ClientHandle> {
        let config = recovering.config().await;
        validate_scope_config(scope, &config)?;
        let required_mints = required_mint_modules(&config)?;
        ensure_mint_recoveries_started(database, &required_mints).await?;
        recovering
            .wait_for_all_recoveries()
            .await
            .with_context(|| format!("wait for {operation}"))?;
        ensure_mint_recoveries_finished(database, &required_mints).await?;
        recovering.shutdown().await;

        let client = client_builder(guardian_key)
            .await?
            .open(
                connectors().await?,
                database.clone(),
                RootSecret::StandardDoubleDerive(client_root.clone()),
            )
            .await
            .with_context(|| format!("reopen completed {operation}"))?;
        validate_scope_config(scope, &client.config().await)?;
        // Mint recovery can create output state machines. Their notes are not
        // spendable until they finish, so do not publish a premature balance.
        client
            .wait_for_all_active_state_machines()
            .await
            .with_context(|| format!("settle recovered mint outputs for {operation}"))?;
        mark_recovery_ready(database).await?;
        Ok(client)
    }

    async fn guardian_fee_client(
        &self,
        invite_code: &InviteCode,
        seat_id: &fedi_decentralized_service_fleet_manager::SeatId,
        key: &GuardianFeeAccountKey,
    ) -> anyhow::Result<ClientHandleArc> {
        let scope = ClientScope::Guardian {
            federation_id: invite_code.federation_id(),
            seat_id: seat_id.to_string(),
        };
        self.join_with_timeout(invite_code, scope.clone(), JOIN_ATTEMPT_TIMEOUT, Some(key))
            .await?;
        self.federations
            .read()
            .await
            .get(&scope)
            .cloned()
            .context("open guardian-fee client")
    }

    async fn join_exclusion(&self, scope: ClientScope) -> tokio::sync::OwnedMutexGuard<()> {
        let join_lock = {
            let mut locks = self.join_locks.lock().await;
            locks
                .entry(scope)
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        join_lock.lock_owned().await
    }

    /// Exclude concurrent payout starts in one wallet scope.
    async fn payout_exclusion(&self, scope: ClientScope) -> tokio::sync::OwnedMutexGuard<()> {
        let payout_lock = {
            let mut locks = self.payout_locks.lock().await;
            locks
                .entry(scope)
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        payout_lock.lock_owned().await
    }

    /// Exclude concurrent collection snapshots in one guardian wallet scope.
    async fn collection_exclusion(&self, scope: ClientScope) -> tokio::sync::OwnedMutexGuard<()> {
        let collection_lock = {
            let mut locks = self.collection_locks.lock().await;
            locks
                .entry(scope)
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        collection_lock.lock_owned().await
    }

    /// Receive an OOB ecash token: validate it belongs to `federation_id`,
    /// reissue it into our wallet, and return the received amount. Returns
    /// only after reissuance completed — an unclaimable token is an error,
    /// never a deferred receive.
    pub async fn receive(
        &self,
        federation_id: FederationId,
        raw_token: &str,
    ) -> Result<Amount, WalletError> {
        let notes: OOBNotes = raw_token
            .trim()
            .parse()
            .map_err(WalletError::InvalidToken)?;
        if notes.federation_id_prefix() != federation_id.to_prefix() {
            return Err(WalletError::WrongFederation {
                selected: federation_id,
            });
        }
        let amount = notes.total_amount();

        let client = self.client(federation_id).await?;
        let mint = client
            .get_first_module::<MintClientModule>()
            .context("mint module")?;
        let operation_id = match mint.reissue_external_notes(notes.clone(), ()).await {
            Ok(operation_id) => operation_id,
            // These notes were already submitted — an earlier receive was
            // interrupted after its reissue went in. Resume that operation
            // instead of failing: the reissue operation id is deterministic
            // in the notes, so the retry can find it (see
            // [`reissue_operation_id`]).
            Err(err)
                if matches!(
                    err.downcast_ref::<ReissueExternalNotesError>(),
                    Some(ReissueExternalNotesError::AlreadyReissued)
                ) =>
            {
                reissue_operation_id(&notes)
            }
            Err(err) => return Err(WalletError::ReceiveFailed(format!("{err:#}"))),
        };
        let mut updates = mint
            .subscribe_reissue_external_notes(operation_id)
            .await
            .context("subscribe to reissue")?
            .into_stream();
        while let Some(state) = updates.next().await {
            match state {
                ReissueExternalNotesState::Done => return Ok(amount),
                ReissueExternalNotesState::Failed(reason) => {
                    return Err(WalletError::ReceiveFailed(reason));
                }
                ReissueExternalNotesState::Created | ReissueExternalNotesState::Issuing => {}
            }
        }
        Err(WalletError::ReceiveFailed(
            "reissue update stream ended before completion".to_owned(),
        ))
    }

    /// Receive an encoded mint-v2 `ECash` token into ordinary balance.
    pub async fn receive_v2(
        &self,
        federation_id: FederationId,
        token: &str,
    ) -> Result<Amount, WalletError> {
        let client = self.client(federation_id).await?;
        let mint = client
            .get_first_module::<MintV2ClientModule>()
            .context("mint-v2 module")?;
        let ecash: MintV2Ecash =
            fedimint_core::base32::decode_prefixed(fedimint_core::base32::FEDIMINT_PREFIX, token)
                .context("decode mint-v2 ecash")?;
        if ecash.mint() != Some(federation_id) {
            return Err(anyhow::anyhow!("mint-v2 token belongs to another federation").into());
        }
        self.receive_v2_notes(federation_id, &mint, ecash.notes())
            .await
    }

    /// Current ecash balance in one federation.
    pub async fn balance(&self, federation_id: FederationId) -> Result<Amount, WalletError> {
        let client = self.client(federation_id).await?;
        Ok(client.get_balance_for_btc().await?)
    }

    /// Every federation this wallet has joined.
    pub(crate) async fn federation_ids(&self) -> Vec<FederationId> {
        self.federations
            .read()
            .await
            .keys()
            .filter_map(|scope| match scope {
                ClientScope::Payment(id) => Some(*id),
                ClientScope::Guardian { .. } => None,
            })
            .collect()
    }

    /// Whether a payment client is already open in this process.
    ///
    /// Public quote handlers must fail fast while policy reconciliation is
    /// joining or recovering a client; they must never wait on join exclusion.
    pub(crate) async fn payment_client_is_open(&self, federation_id: FederationId) -> bool {
        self.federations
            .read()
            .await
            .contains_key(&ClientScope::Payment(federation_id))
    }

    /// Every durable payment scope, whether or not its client is open now.
    pub(crate) async fn retained_federation_ids(&self) -> Vec<FederationId> {
        self.prefixes
            .read()
            .await
            .keys()
            .filter_map(|scope| match scope {
                ClientScope::Payment(id) => Some(*id),
                ClientScope::Guardian { .. } => None,
            })
            .collect()
    }

    /// Configured mint-v1 note denominations for a joined federation.
    pub(crate) async fn mint_denominations(
        &self,
        federation_id: FederationId,
    ) -> Result<Vec<Amount>, WalletError> {
        let client = self.client(federation_id).await?;
        let mint = client
            .get_first_module::<MintClientModule>()
            .context("mint module")?;
        Ok(mint.context().tbs_pks.tiers().copied().collect())
    }

    /// Economical mint-v2 denominations for an exact selected module.
    pub(crate) async fn mint_v2_denominations(
        &self,
        federation_id: FederationId,
        mint_module: ModuleInstanceId,
    ) -> Result<Vec<MintV2Denomination>, WalletError> {
        let client = self.client(federation_id).await?;
        let _ = mint_v2_module(&client, mint_module)?;
        Ok(fedimint_mintv2_common::config::consensus_denominations()
            .filter(|denomination| denomination.amount().msats > 100)
            .collect())
    }

    pub async fn first_mint_v2_module_id(
        &self,
        federation_id: FederationId,
    ) -> Result<ModuleInstanceId, WalletError> {
        let client = self.client(federation_id).await?;
        Ok(client
            .get_first_module::<MintV2ClientModule>()
            .context("mint-v2 module")?
            .id)
    }

    /// Whether the joined federation's consensus config carries a mintv2
    /// module. Errors only when the federation is not joined; module absence
    /// is `Ok(false)`.
    pub(crate) async fn has_mint_v2_module(
        &self,
        federation_id: FederationId,
    ) -> Result<bool, WalletError> {
        let client = self.client(federation_id).await?;
        Ok(client.get_first_module::<MintV2ClientModule>().is_ok())
    }

    /// Run the shared refund preparation under this wallet's own secrets
    /// (see [`refund::prepare_quote_refund`]); the payee uses the result as
    /// its validation oracle for a presented commitment.
    pub(crate) async fn prepare_quote_refund(
        &self,
        federation_id: FederationId,
        price_msats: u64,
        refund_nonce: [u8; 32],
    ) -> Result<fedi_decentralized_service_fleet_manager::RefundIssuance, WalletError> {
        let client = self.client(federation_id).await?;
        refund::prepare_quote_refund(
            &client,
            &self.root_secret,
            self.wallet_secret.expose_for_derivation(),
            federation_id,
            price_msats,
            refund_nonce,
        )
        .await
        .map_err(Into::into)
    }

    async fn receive_v2_notes(
        &self,
        federation_id: FederationId,
        mint: &MintV2ClientModule,
        notes: Vec<fedimint_mintv2_client::SpendableNote>,
    ) -> Result<Amount, WalletError> {
        let ecash = MintV2Ecash::new(federation_id, notes);
        let amount = ecash.amount();
        let operation_log_client = self.client(federation_id).await?;
        let operation_id = payee::handoff_mint_v2_receive(
            &ecash,
            || mint.receive(ecash.clone(), serde_json::Value::Null),
            |operation_id| async move {
                operation_log_client
                    .operation_log()
                    .get_operation(operation_id)
                    .await
            },
        )
        .await
        .context("start mint-v2 reissue")?;
        match mint
            .await_final_receive_operation_state(operation_id)
            .await
            .context("await mint-v2 reissue")?
        {
            MintV2ReceiveState::Success => Ok(amount),
            MintV2ReceiveState::Rejected => Err(anyhow::anyhow!("mint-v2 reissue rejected").into()),
        }
    }

    async fn client(&self, federation_id: FederationId) -> Result<ClientHandleArc, WalletError> {
        let scope = ClientScope::Payment(federation_id);
        if let Some(client) = self.federations.read().await.get(&scope).cloned() {
            return Ok(client);
        }
        let _opening = self.join_exclusion(scope.clone()).await;
        if let Some(client) = self.federations.read().await.get(&scope).cloned() {
            return Ok(client);
        }
        let prefix = self
            .prefixes
            .read()
            .await
            .get(&scope)
            .copied()
            .ok_or(WalletError::UnknownFederation(federation_id))?;
        let database = self.database.with_prefix(prefix.consensus_encode_to_vec());
        if !Client::is_initialized(&database).await {
            return Err(anyhow::anyhow!("retained payment scope has no initialized client").into());
        }
        let client_root = self.client_root(&scope)?;
        let client = client_open_once(&self.open_attempted, scope.clone(), async {
            self.open_initialized_scope(
                &scope,
                &database,
                &client_root,
                None,
                "reopen retained payment federation",
            )
            .await
        })
        .await?;
        validate_scope_config(&scope, &client.config().await)?;
        validate_scope_prefix(prefix, &scope, client.federation_id())?;
        let client = Arc::new(client);
        self.federations.write().await.insert(scope, client.clone());
        Ok(client)
    }
}

async fn client_open_once<T>(
    attempted: &Mutex<HashSet<ClientScope>>,
    scope: ClientScope,
    open: impl std::future::Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    anyhow::ensure!(
        attempted.lock().await.insert(scope),
        "wallet client open was already attempted; restart before retrying"
    );
    open.await
}

/// Refuse a database whose contents disagree with the map that chose its
/// prefix. Kept a named function rather than inlined: it is the only check
/// that our prefix map and the Fedimint state under it still agree, and the
/// boundary is what lets a test pin that refusal without a live federation.
fn validate_scope_prefix(
    prefix: u64,
    scope: &ClientScope,
    actual: FederationId,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        actual == scope.federation_id(),
        "client scope prefix {prefix} contains a different federation"
    );
    Ok(())
}

/// Validate the persisted identity before an ordinary open can resume recovery.
async fn validate_stored_scope(database: &Database, scope: &ClientScope) -> anyhow::Result<()> {
    let config = Client::get_config_from_db(database)
        .await
        .context("initialized client has no stored federation config")?;
    validate_scope_config(scope, &config)
}

/// Return whether a recover-mode client still lacks the FMan readiness marker.
async fn recovery_needs_ready_marker(database: &Database) -> anyhow::Result<bool> {
    let mut dbtx = database.begin_transaction_nc().await;
    let state = dbtx
        .get_value(&ClientInitStateKey)
        .await
        .context("initialized client has no initialization state")?;
    anyhow::ensure!(
        !state.is_pending() || state.does_require_recovery().is_some(),
        "client is pending a fresh initialization, not a recoverable client"
    );
    if matches!(
        &state,
        fedimint_client::db::InitState::Complete(fedimint_client::db::InitModeComplete::Recover)
    ) {
        drop(dbtx);
        return Ok(!recovery_ready(database).await?);
    }
    Ok(state.does_require_recovery().is_some())
}

/// FMan's durable postcondition for a recovery that upstream calls complete.
const RECOVERY_READY_KEY: &[u8] = b"\xb0fman/recovery-ready/v1";

/// Read the per-client readiness marker without accepting malformed local data.
async fn recovery_ready(database: &Database) -> anyhow::Result<bool> {
    let mut dbtx = database.begin_transaction_nc().await;
    match dbtx.raw_get_bytes(RECOVERY_READY_KEY).await? {
        None => Ok(false),
        Some(value) => {
            anyhow::ensure!(
                value.as_slice() == [1],
                "invalid federation recovery readiness marker"
            );
            Ok(true)
        }
    }
}

/// Mark a recovered client ready only after its mint outputs have settled.
async fn mark_recovery_ready(database: &Database) -> anyhow::Result<()> {
    let mut dbtx = database.begin_transaction().await;
    dbtx.raw_insert_bytes(RECOVERY_READY_KEY, &[1]).await?;
    dbtx.commit_tx().await;
    Ok(())
}

/// Require the supported mint configuration for a scope before recovery starts.
fn validate_scope_config(
    scope: &ClientScope,
    config: &fedimint_core::config::ClientConfig,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        config.calculate_federation_id() == scope.federation_id(),
        "client config belongs to a different federation than its scope"
    );
    if matches!(scope, ClientScope::Payment(_)) {
        validate_payment_config(config)?;
    }
    required_mint_modules(config)?;
    Ok(())
}

/// Return every configured mint whose historical notes this client must scan.
fn required_mint_modules(
    config: &fedimint_core::config::ClientConfig,
) -> anyhow::Result<BTreeSet<ModuleInstanceId>> {
    let mints = config
        .modules
        .iter()
        .filter_map(|(id, module)| {
            (module.kind == fedimint_mint_common::KIND
                || module.kind == fedimint_mintv2_common::KIND)
                .then_some(*id)
        })
        .collect::<BTreeSet<_>>();
    anyhow::ensure!(
        !mints.is_empty(),
        "federation has no supported mint module to recover"
    );
    Ok(mints)
}

/// Refuse success when Fedimint skipped a configured mint during recovery setup.
async fn ensure_mint_recoveries_started(
    database: &Database,
    required_mints: &BTreeSet<ModuleInstanceId>,
) -> anyhow::Result<()> {
    let mut dbtx = database.begin_transaction_nc().await;
    for module_instance_id in required_mints {
        anyhow::ensure!(
            dbtx.get_value(&ClientModuleRecovery {
                module_instance_id: *module_instance_id,
            })
            .await
            .is_some(),
            "Fedimint did not start recovery for required mint module {module_instance_id}"
        );
    }
    Ok(())
}

/// Refuse success unless each required mint reports durable recovery completion.
async fn ensure_mint_recoveries_finished(
    database: &Database,
    required_mints: &BTreeSet<ModuleInstanceId>,
) -> anyhow::Result<()> {
    let mut dbtx = database.begin_transaction_nc().await;
    for module_instance_id in required_mints {
        let recovery = dbtx
            .get_value(&ClientModuleRecovery {
                module_instance_id: *module_instance_id,
            })
            .await
            .with_context(|| {
                format!("required mint module {module_instance_id} lost its recovery state")
            })?;
        anyhow::ensure!(
            recovery.is_done(),
            "required mint module {module_instance_id} did not complete recovery"
        );
    }
    Ok(())
}

/// Domain-separate guardian seats before Fedimint adds its federation and module paths.
fn guardian_scope_root(root: &DerivableSecret, seat_id: &str) -> DerivableSecret {
    root.tweak(
        [
            b"fman/guardian-fee-client/seat/v1/".as_slice(),
            seat_id.as_bytes(),
        ]
        .concat()
        .as_slice(),
    )
}

const GLOBAL_DB_PREFIX: u64 = 0;
const NEXT_PREFIX_KEY: &[u8] = b"next-client-prefix";
const PREFIX_FEDERATION_KEY: u8 = 1;

fn global_db(database: &Database) -> Database {
    database.with_prefix(GLOBAL_DB_PREFIX.consensus_encode_to_vec())
}

fn scope_key(prefix: u64) -> Vec<u8> {
    let mut key = vec![PREFIX_FEDERATION_KEY];
    key.extend(prefix.consensus_encode_to_vec());
    key
}

async fn load_prefixes(database: &Database) -> anyhow::Result<BTreeMap<ClientScope, u64>> {
    let global = global_db(database);
    let mut tx = global.begin_transaction_nc().await;
    let mut entries = tx.raw_find_by_prefix(&[PREFIX_FEDERATION_KEY]).await?;
    let mut prefixes = BTreeMap::new();
    while let Some((key, value)) = entries.next().await {
        anyhow::ensure!(key.len() > 1, "invalid federation prefix mapping key");
        let prefix = u64::consensus_decode_whole(&key[1..], &ModuleDecoderRegistry::default())?;
        anyhow::ensure!(
            prefix != 0,
            "federation prefix 0 is reserved for global data"
        );
        let scope = ClientScope::consensus_decode_whole(&value, &ModuleDecoderRegistry::default())?;
        anyhow::ensure!(
            prefixes.insert(scope, prefix).is_none(),
            "duplicate client scope mapping"
        );
    }
    Ok(prefixes)
}

async fn reserve_prefix(database: &Database, scope: &ClientScope) -> anyhow::Result<u64> {
    let global = global_db(database);
    let mut tx = global.begin_transaction().await;
    let next = match tx.raw_get_bytes(NEXT_PREFIX_KEY).await? {
        Some(value) => u64::consensus_decode_whole(&value, &ModuleDecoderRegistry::default())?,
        None => 1,
    };
    anyhow::ensure!(next != 0, "federation prefix counter wrapped");
    let following = next
        .checked_add(1)
        .context("federation prefix counter exhausted")?;
    tx.raw_insert_bytes(NEXT_PREFIX_KEY, &following.consensus_encode_to_vec())
        .await?;
    tx.raw_insert_bytes(&scope_key(next), &scope.consensus_encode_to_vec())
        .await?;
    tx.commit_tx().await;
    Ok(next)
}

struct OobReissueTag;
impl sha256t::Tag for OobReissueTag {
    fn engine() -> sha256::HashEngine {
        let mut engine = sha256::HashEngine::default();
        engine.input(b"oob-reissue");
        engine
    }
}
pub(crate) fn reissue_operation_id(notes: &OOBNotes) -> OperationId {
    OperationId(
        notes
            .notes()
            .consensus_hash::<sha256t::Hash<OobReissueTag>>()
            .to_byte_array(),
    )
}

fn ensure_public_invite(invite_code: &InviteCode) -> Result<(), WalletError> {
    if invite_code.api_secret().is_some() {
        return Err(WalletError::PrivateInviteUnsupported);
    }
    Ok(())
}

fn validate_payment_config(config: &fedimint_core::config::ClientConfig) -> anyhow::Result<()> {
    let mints = config
        .modules
        .iter()
        .filter(|(_, module)| {
            module.kind == fedimint_mint_common::KIND || module.kind == fedimint_mintv2_common::KIND
        })
        .collect::<Vec<_>>();
    anyhow::ensure!(
        mints.len() == 1,
        "payment federation must have exactly one supported Bitcoin mint generation"
    );
    let (module_id, module) = mints[0];
    if module.kind == fedimint_mintv2_common::KIND {
        // Configs downloaded by ClientPreview retain raw dynamic module bytes
        // until join. Decode this module before validating it so rejection can
        // still precede the first client database write.
        let decoders = ModuleDecoderRegistry::new([(
            *module_id,
            fedimint_mintv2_common::KIND,
            fedimint_mintv2_common::MintCommonInit::decoder(),
        )]);
        let module = module
            .clone()
            .redecode_raw(&decoders)?
            .config
            .decoded()
            .context("decode mint-v2 client config")?;
        let mint = module
            .as_any()
            .downcast_ref::<fedimint_mintv2_common::config::MintClientConfig>()
            .context("decode mint-v2 client config")?;
        anyhow::ensure!(
            mint.amount_unit == fedimint_core::module::AmountUnit::BITCOIN,
            "payment federation's mint-v2 module is not a Bitcoin mint"
        );
    }
    Ok(())
}

async fn lnurl_pay(
    destination: &str,
    choose_amount: impl FnOnce(u64) -> u64,
) -> anyhow::Result<(Bolt11Invoice, u64)> {
    let lnurl = if destination.contains('@') {
        LightningAddress::from_str(destination)
            .context("invalid Lightning Address")?
            .lnurl()
    } else {
        LnUrl::from_str(destination).context("invalid LNURL-pay destination")?
    };
    let client = lnurl::Builder::default().timeout(30).build_async()?;
    let pay_response = bounded_lnurl_get(&client, &lnurl.url, None).await?;
    let LnUrlResponse::LnUrlPayResponse(pay) =
        lnurl::decode_ln_url_response(std::str::from_utf8(&pay_response)?)?
    else {
        anyhow::bail!("destination is not an LNURL-pay endpoint");
    };
    let amount = choose_amount(pay.max_sendable);
    anyhow::ensure!(
        amount >= pay.min_sendable,
        "balance cannot cover the destination's minimum payment and fees"
    );
    let response = bounded_lnurl_get(&client, &pay.callback, Some(amount)).await?;
    let response: lnurl::pay::LnURLPayInvoice = serde_json::from_slice(&response)?;
    let invoice = Bolt11Invoice::from_str(response.invoice()).context("invalid LNURL invoice")?;
    anyhow::ensure!(
        invoice.amount_milli_satoshis() == Some(amount),
        "LNURL endpoint returned an invoice for the wrong amount"
    );
    Ok((invoice, amount))
}

/// Fetch one LNURL response without retaining more than the compatibility cap.
async fn bounded_lnurl_get(
    client: &lnurl::AsyncClient,
    url: &str,
    amount: Option<u64>,
) -> anyhow::Result<Vec<u8>> {
    let mut request = client.client.get(url);
    if let Some(amount) = amount {
        request = request.query(&[("amount", amount)]);
    }
    let mut response = request.send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_LNURL_RESPONSE_BYTES as u64)
    {
        anyhow::bail!("LNURL response exceeds {MAX_LNURL_RESPONSE_BYTES} bytes");
    }

    let mut body = Vec::with_capacity(
        response
            .content_length()
            .and_then(|length| usize::try_from(length).ok())
            .unwrap_or_default(),
    );
    while let Some(chunk) = response.chunk().await? {
        anyhow::ensure!(
            chunk.len() <= MAX_LNURL_RESPONSE_BYTES - body.len(),
            "LNURL response exceeds {MAX_LNURL_RESPONSE_BYTES} bytes"
        );
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Select the exact Lightning v1 gateway used by both sweep execution and its
/// point-in-time affordability projection.
pub(crate) async fn select_v1_gateway(
    client: &ClientHandleArc,
    lightning: &LightningClientModule,
) -> anyhow::Result<fedimint_ln_common::LightningGateway> {
    lightning.update_gateway_cache().await?;
    let vetted = client
        .meta_service()
        .get_field::<Vec<String>>(client.db(), "vetted_gateways")
        .await
        .map_or(Vec::new(), |value| value.value.unwrap_or_default())
        .into_iter()
        .filter_map(|value| {
            bitcoin::secp256k1::PublicKey::from_str(&value)
                .inspect_err(|error| warn!(%error, %value, "invalid vetted gateway"))
                .ok()
        })
        .collect::<HashSet<_>>();
    let announcements = lightning.list_gateways().await;
    let (mut vetted_gateways, unvetted_gateways): (Vec<_>, Vec<_>) = announcements
        .iter()
        .cloned()
        .into_iter()
        .filter(|announcement| announcement.ttl > MINIMUM_V1_GATEWAY_TTL)
        .partition(|announcement| vetted.contains(&announcement.info.gateway_id));
    if vetted_gateways.is_empty() {
        vetted_gateways = unvetted_gateways;
    }
    vetted_gateways.sort_by_key(|announcement| announcement.info.gateway_id);
    match vetted_gateways.into_iter().next() {
        Some(announcement) => Ok(announcement.info),
        None => Err(v1_gateway_unavailable(
            announcements.iter().map(|announcement| announcement.ttl),
        )),
    }
}

/// Explain why announced legacy gateways cannot safely start a payout.
fn v1_gateway_unavailable(
    announcement_ttls: impl Iterator<Item = std::time::Duration>,
) -> anyhow::Error {
    let announcement_ttls = announcement_ttls.collect::<Vec<_>>();
    let announcement_count = announcement_ttls.len();
    if announcement_count == 0 {
        anyhow::anyhow!("federation announced no Lightning v1 gateways")
    } else if announcement_ttls.iter().all(|ttl| ttl.is_zero()) {
        anyhow::anyhow!(
            "federation announced {} Lightning v1 gateways, but all announcements have expired",
            announcement_count
        )
    } else {
        anyhow::anyhow!(
            "federation announced {} Lightning v1 gateways, but none remains usable for {} seconds",
            announcement_count,
            MINIMUM_V1_GATEWAY_TTL.as_secs()
        )
    }
}

async fn client_builder(
    guardian_key: Option<&GuardianFeeAccountKey>,
) -> anyhow::Result<fedimint_client::ClientBuilder> {
    let mut builder = Client::builder().await?;
    builder.with_module(MintClientInit);
    builder.with_module(MintV2ClientInit);
    builder.with_module(LightningClientInit::default());
    builder.with_module(LightningV2ClientInit::default());
    builder.with_module(MetaClientInit);
    let stability_pool = match guardian_key {
        Some(key) => StabilityPoolClientInit::default().with_btc_depositor_keypair(key.keypair()),
        None => StabilityPoolClientInit::default(),
    };
    builder.with_module(stability_pool);
    Ok(builder)
}

async fn connectors() -> anyhow::Result<ConnectorRegistry> {
    ConnectorRegistry::build_from_client_defaults().bind().await
}

#[cfg(test)]
mod tests;
