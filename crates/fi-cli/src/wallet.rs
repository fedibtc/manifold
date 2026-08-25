//! FI-side payment wallet: one Fedimint client database per joined payment
//! federation, under one wallet secret.
//!
//! Deliberately independent of `fman-fedimint` — that crate is the FMan's.
//! The protocol pieces both roles must agree on come from
//! `locked-payments`; the client plumbing here (join,
//! receive, reissue dedup) is the FI's own, duplicated by design rather
//! than reached through the FMan's crate.
//!
//! Bearer-token hygiene: raw OOB tokens are never logged and never
//! persisted; a received token exists in memory only for the duration of
//! [`Wallet::receive`].

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;
use bitcoin_hashes::{Hash as _, HashEngine as _, sha256, sha256t};
use fedi_decentralized_service_fleet_manager::RefundIssuance;
use fedimint_client::{Client, ClientHandle, ClientHandleArc, RootSecret};
use fedimint_client_module::module::ClientModule as _;
use fedimint_connectors::ConnectorRegistry;
use fedimint_core::Amount;
use fedimint_core::config::FederationId;
use fedimint_core::core::{ModuleInstanceId, OperationId};
use fedimint_core::db::Database;
use fedimint_core::encoding::Encodable as _;
use fedimint_core::invite_code::InviteCode;
use fedimint_core::module::AmountUnit;
use fedimint_derive_secret::DerivableSecret;
use fedimint_ln_client::receive::LightningReceiveError as LightningV1ReceiveError;
use fedimint_ln_client::{
    LightningClientInit as LightningV1ClientInit, LightningClientModule as LightningV1ClientModule,
    LnReceiveState as LightningV1ReceiveState,
};
use fedimint_lnv2_client::common::Bolt11InvoiceDescription;
use fedimint_lnv2_client::{
    FinalReceiveOperationState as LightningV2ReceiveState,
    LightningClientInit as LightningV2ClientInit, LightningClientModule as LightningV2ClientModule,
    ReceiveError as LightningV2ReceiveError, SelectGatewayError as LightningV2SelectGatewayError,
};
use fedimint_mint_client::{
    MintClientInit, MintClientModule, OOBNotes, ReissueExternalNotesError,
    ReissueExternalNotesState,
};
use fedimint_mintv2_client::MintClientInit as MintV2ClientInit;
use fedimint_mintv2_client::{
    ECash as MintV2Ecash, FinalReceiveOperationState as MintV2ReceiveState,
    MintClientModule as MintV2ClientModule, MintOperationMeta as MintV2OperationMeta,
    ReceiveECashError as MintV2ReceiveError,
};
use fedimint_wallet_client::{
    WalletClientInit as WalletV1ClientInit, WalletClientModule as WalletV1ClientModule,
};
use fedimint_walletv2_client::{
    WalletClientInit as WalletV2ClientInit, WalletClientModule as WalletV2ClientModule,
};
use futures::StreamExt as _;
use lightning_invoice::{Bolt11InvoiceDescription as Bolt11InvoiceDescriptionV1, Description};
use locked_payments::refund::{self, PreparedRefund, PreparedRefundV2, RefundError};
use locked_payments::{locked_payment, locked_payment_v2, mint_v2_module};
use stability_pool_client::common::{AccountId, BtcBalanceDepositMetadata};
use stability_pool_client::{
    StabilityPoolClientInit, StabilityPoolClientModule, StabilityPoolDepositOperationState,
};
use tokio::sync::{Mutex, RwLock};
use zeroize::Zeroizing;

const WALLET_SECRET_SALT: &[u8] = b"fi-payment-wallet-v0";
const WALLET_LOCK_FILE: &str = ".wallet.lock";
const JOIN_ATTEMPT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Stable terminal projection shared by legacy LN and LN-v2 receive operations.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum PaymentWalletInvoiceState {
    Claimed,
    Expired,
    Failure,
}

/// FI wallet root secret bytes.
pub struct WalletSecret(pub [u8; 64]);

/// Wallet failures; the payment driver above maps them onto its own
/// reporting.
#[derive(Debug, thiserror::Error)]
pub enum WalletError {
    /// The federation is not joined by this wallet.
    #[error("federation {0} is not joined")]
    UnknownFederation(FederationId),

    /// The submitted string is not a parsable OOB ecash token.
    #[error("invalid ecash token")]
    InvalidToken(#[source] anyhow::Error),

    /// The token was minted by a different federation than the one selected
    /// for the payment.
    #[error("token does not belong to federation {selected}")]
    WrongFederation {
        /// Federation selected for the payment.
        selected: FederationId,
    },

    /// The federation refused reissuance (double-spent/invalid notes) or
    /// could not be reached to complete it.
    #[error("ecash receive failed: {0}")]
    ReceiveFailed(String),

    /// The soft attempt budget elapsed.
    #[error("wallet federation {federation_id} join timed out")]
    JoinTimedOut {
        /// Federation whose attempt exceeded the reporting budget.
        federation_id: FederationId,
    },

    /// Reopening an already-joined federation exceeded the attempt budget.
    #[error("wallet federation {federation_id} open timed out")]
    OpenTimedOut {
        /// Federation whose attempt exceeded the reporting budget.
        federation_id: FederationId,
    },

    /// Waiting for direct funding exceeded the caller's deadline.
    #[error(
        "wallet federation {federation_id} did not reach {minimum_msats} msat before the funding deadline"
    )]
    FundingTimedOut {
        /// Federation whose balance was observed.
        federation_id: FederationId,
        /// Minimum balance requested by the caller.
        minimum_msats: u64,
    },

    /// Deriving a wallet-v2 receive address exceeded the caller's deadline.
    #[error("wallet federation {federation_id} deposit address timed out")]
    DepositAddressTimedOut {
        /// Federation whose wallet-v2 scanner did not produce an address.
        federation_id: FederationId,
    },

    /// Any other client failure (open, join, receive).
    #[error(transparent)]
    Client(#[from] anyhow::Error),

    /// Key-locked payment evidence or refund outputs are invalid.
    #[error("invalid key-locked payment: {0}")]
    InvalidLockedPayment(#[from] locked_payment::LockedPaymentError),

    /// Mint-v2 key-locked payment evidence or refund outputs are invalid.
    #[error("invalid mint-v2 key-locked payment: {0}")]
    InvalidLockedPaymentV2(#[from] locked_payment_v2::LockedPaymentV2Error),
    #[error(
        "guardian-fee remittance {operation_id} was accepted, but its change output failed: {reason}; do not retry this remittance"
    )]
    GuardianFeeRemittanceChangeFailed {
        operation_id: String,
        reason: String,
    },
}

impl From<RefundError> for WalletError {
    fn from(error: RefundError) -> Self {
        match error {
            RefundError::V1(error) => Self::InvalidLockedPayment(error),
            RefundError::V2(error) => Self::InvalidLockedPaymentV2(error),
            RefundError::Client(error) => Self::Client(error),
        }
    }
}

/// One Fedimint client per joined payment federation, each in its own
/// RocksDB under the wallet data directory, all derived from one root.
pub struct Wallet {
    data_dir: PathBuf,
    /// Wallet-wide FI aggregate reservation journal. This is separate from
    /// every joined federation client database and contains no bearer ecash.
    pub(crate) database: Database,
    root_secret: DerivableSecret,
    wallet_secret: Zeroizing<[u8; 64]>,
    federations: RwLock<BTreeMap<FederationId, ClientHandleArc>>,
    /// Serializes aggregate planning, reservation transitions, and payment
    /// submission for this process-exclusive development wallet.
    pub(crate) spend_guard: Mutex<()>,
    /// One join at a time; joins are rare and serialization keeps the
    /// per-federation database creation race-free.
    join_lock: Mutex<()>,
    /// Exclusive, non-symlink-following lock on the wallet root, held until
    /// the wallet drops, so two fi-cli invocations cannot share state.
    _data_dir_lock: Arc<std::fs::File>,
}

impl Wallet {
    /// Open the wallet root. Per-federation secrets are derived from
    /// `secret` by fedimint-client's standard double-derivation, so all
    /// federation wallets are recoverable from the root alone.
    pub async fn open(data_dir: PathBuf, secret: &WalletSecret) -> anyhow::Result<Self> {
        let root_secret = DerivableSecret::new_root(&secret.0, WALLET_SECRET_SALT);
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
        let database: Database =
            fedimint_rocksdb::RocksDb::build(data_dir.join("payment-reservations"))
                .open()
                .await
                .context("open FI payment reservation journal")?
                .into();
        Ok(Self {
            data_dir,
            database,
            root_secret,
            wallet_secret: Zeroizing::new(secret.0),
            federations: RwLock::new(BTreeMap::new()),
            spend_guard: Mutex::new(()),
            join_lock: Mutex::new(()),
            _data_dir_lock: Arc::new(data_dir_lock),
        })
    }

    /// Join a payment federation (or reopen its existing client database).
    pub async fn join(&self, invite_code: &InviteCode) -> Result<FederationId, WalletError> {
        let federation_id = invite_code.federation_id();
        match tokio::time::timeout(JOIN_ATTEMPT_TIMEOUT, self.join_inner(invite_code)).await {
            Ok(result) => result,
            Err(_) => Err(WalletError::JoinTimedOut { federation_id }),
        }
    }

    /// Open a federation client that this wallet joined in an earlier process.
    ///
    /// Unlike [`Self::join`], this accepts no invite and never initializes a
    /// new client database. The federation id selects the existing database
    /// underneath this wallet root.
    pub async fn open_federation(
        &self,
        federation_id: FederationId,
    ) -> Result<FederationId, WalletError> {
        match tokio::time::timeout(
            JOIN_ATTEMPT_TIMEOUT,
            self.open_federation_inner(federation_id),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Err(WalletError::OpenTimedOut { federation_id }),
        }
    }

    async fn open_federation_inner(
        &self,
        federation_id: FederationId,
    ) -> Result<FederationId, WalletError> {
        let _joining = self.join_lock.lock().await;
        if self.federations.read().await.contains_key(&federation_id) {
            return Ok(federation_id);
        }
        let database_path = self.data_dir.join(federation_id.to_string());
        if !database_path.exists() {
            return Err(WalletError::UnknownFederation(federation_id));
        }
        let database: Database = fedimint_rocksdb::RocksDb::build(database_path)
            .open()
            .await
            .context("open federation client rocksdb")?
            .into();
        if !Client::is_initialized(&database).await {
            return Err(WalletError::UnknownFederation(federation_id));
        }
        let client = self.open_initialized_client(database).await?;
        self.insert_client(federation_id, client).await
    }

    async fn join_inner(&self, invite_code: &InviteCode) -> Result<FederationId, WalletError> {
        let federation_id = invite_code.federation_id();
        let _joining = self.join_lock.lock().await;
        if self.federations.read().await.contains_key(&federation_id) {
            return Ok(federation_id);
        }
        let database: Database =
            fedimint_rocksdb::RocksDb::build(self.data_dir.join(federation_id.to_string()))
                .open()
                .await
                .context("open federation client rocksdb")?
                .into();
        let client = if Client::is_initialized(&database).await {
            self.open_initialized_client(database).await?
        } else {
            let root = RootSecret::StandardDoubleDerive(self.root_secret.clone());
            let preview = client_builder()
                .await?
                .preview(connectors().await?, invite_code)
                .await
                .context("download federation config from invite code")?;
            preview
                .join(database, root)
                .await
                .context("join federation")?
        };
        self.insert_client(federation_id, client).await
    }

    async fn open_initialized_client(&self, database: Database) -> anyhow::Result<ClientHandle> {
        client_builder()
            .await?
            .open(
                connectors().await?,
                database,
                RootSecret::StandardDoubleDerive(self.root_secret.clone()),
            )
            .await
            .context("open joined federation")
    }

    async fn insert_client(
        &self,
        federation_id: FederationId,
        client: ClientHandle,
    ) -> Result<FederationId, WalletError> {
        if client.federation_id() != federation_id {
            return Err(anyhow::anyhow!(
                "federation client database contains a different federation"
            )
            .into());
        }
        self.federations
            .write()
            .await
            .insert(federation_id, Arc::new(client));
        Ok(federation_id)
    }

    /// Return the spendable Bitcoin-unit balance of a joined payment federation.
    pub async fn balance(&self, federation_id: FederationId) -> Result<Amount, WalletError> {
        self.client(federation_id)
            .await?
            .get_balance_for_btc()
            .await
            .context("read payment wallet balance")
            .map_err(Into::into)
    }

    /// Wait until the spendable Bitcoin-unit balance reaches `minimum`.
    pub async fn wait_for_balance(
        &self,
        federation_id: FederationId,
        minimum: Amount,
        timeout: std::time::Duration,
    ) -> Result<Amount, WalletError> {
        let client = self.client(federation_id).await?;
        let mut balances = client.subscribe_balance_changes(AmountUnit::BITCOIN).await;
        let reached = tokio::time::timeout(timeout, async {
            while let Some(balance) = balances.next().await {
                if balance >= minimum {
                    return Some(balance);
                }
            }
            None
        })
        .await
        .map_err(|_| WalletError::FundingTimedOut {
            federation_id,
            minimum_msats: minimum.msats,
        })?;
        reached.ok_or_else(|| {
            WalletError::Client(anyhow::anyhow!(
                "payment wallet balance stream ended before reaching the requested minimum"
            ))
        })
    }

    /// Return an on-chain address whose available wallet module credits this client.
    pub async fn deposit_address(
        &self,
        federation_id: FederationId,
        timeout: std::time::Duration,
    ) -> Result<String, WalletError> {
        let client = self.client(federation_id).await?;
        if let Ok(wallet) = client.get_first_module::<WalletV2ClientModule>() {
            let address = tokio::time::timeout(timeout, wallet.receive())
                .await
                .map_err(|_| WalletError::DepositAddressTimedOut { federation_id })?;
            return Ok(address.to_string());
        }

        let wallet = client
            .get_first_module::<WalletV1ClientModule>()
            .context("wallet module")?;
        let address = wallet
            .allocate_deposit_address_expert_only(
                serde_json::json!({ "purpose": "fi-cli-payment-wallet-funding" }),
            )
            .await
            .context("allocate wallet funding address")?;
        Ok(address.address.to_string())
    }

    /// Submit the payer side of a guardian-fee remittance to an explicit
    /// BtcDepositor account using the stability-pool module's production
    /// deposit operation.
    pub async fn deposit_to_btc_balance(
        &self,
        federation_id: FederationId,
        account_id: AccountId,
        amount: Amount,
        metadata: Vec<u8>,
    ) -> Result<OperationId, WalletError> {
        let spend_guard = self.spend_guard.lock().await;
        let holds = self.locked_payment_hold_summary(federation_id).await?;
        if holds.held_msats != 0 || holds.required_reserve_msats != 0 {
            return Err(anyhow::anyhow!(
                "guardian-fee remittance is unavailable while locked-payment value is reserved"
            )
            .into());
        }
        let client = self.client(federation_id).await?;
        let stability_pool = client
            .get_first_module::<StabilityPoolClientModule>()
            .context("stability-pool module")?;
        let operation_id = stability_pool
            .deposit_to_btc_balance(
                account_id,
                amount,
                BtcBalanceDepositMetadata(metadata),
                serde_json::json!({ "purpose": "fi-cli-guardian-fee-remittance" }),
            )
            .await
            .context("submit guardian-fee remittance")?;
        // The native operation is durably committed now; later observation
        // cannot consume a second allocation, so ordinary spending may resume.
        drop(spend_guard);
        let mut updates = stability_pool
            .subscribe_deposit_operation(operation_id)
            .await
            .context("subscribe to guardian-fee remittance")?
            .into_stream();
        let mut terminal = None;
        while let Some(update) = updates.next().await {
            match update {
                // Poll through the terminal item so Fedimint's caching stream
                // durably records it before this process exits.
                state @ (StabilityPoolDepositOperationState::Success
                | StabilityPoolDepositOperationState::TxRejected(_)
                | StabilityPoolDepositOperationState::PrimaryOutputError(_)) => {
                    terminal = Some(state);
                }
                _ => {}
            }
        }
        match terminal {
            Some(StabilityPoolDepositOperationState::Success) => Ok(operation_id),
            Some(StabilityPoolDepositOperationState::TxRejected(reason)) => {
                Err(anyhow::anyhow!("guardian-fee remittance rejected: {reason}").into())
            }
            Some(StabilityPoolDepositOperationState::PrimaryOutputError(reason)) => {
                Err(WalletError::GuardianFeeRemittanceChangeFailed {
                    operation_id: operation_id.fmt_full().to_string(),
                    reason,
                })
            }
            _ => Err(anyhow::anyhow!(
                "guardian-fee remittance state stream ended before a terminal outcome"
            )
            .into()),
        }
    }

    /// Create a Lightning invoice through the joined federation's available LN module.
    ///
    /// LN-v2 is preferred. Its explicit pre-operation `NoGatewaysAvailable`
    /// result permits a legacy-LN fallback; every other LN-v2 error is returned
    /// without starting a second receive path.
    pub async fn create_invoice(
        &self,
        federation_id: FederationId,
        amount: Amount,
        expiry_secs: u32,
    ) -> Result<(String, OperationId), WalletError> {
        let client = self.client(federation_id).await?;
        if let Ok(lightning) = client.get_first_module::<LightningV2ClientModule>() {
            match lightning
                .receive(
                    amount,
                    expiry_secs,
                    Bolt11InvoiceDescription::Direct("fi-cli payment wallet funding".to_owned()),
                    None,
                    serde_json::json!({ "purpose": "fi-cli-payment-wallet-funding" }),
                )
                .await
            {
                Ok((invoice, operation_id)) => {
                    return Ok((invoice.to_string(), operation_id));
                }
                Err(LightningV2ReceiveError::SelectGateway(
                    LightningV2SelectGatewayError::NoGatewaysAvailable,
                )) => {}
                Err(error) => {
                    return Err(anyhow::Error::new(error)
                        .context("create LN-v2 funding invoice")
                        .into());
                }
            }
        }

        let lightning = client
            .get_first_module::<LightningV1ClientModule>()
            .context("ln module")?;
        let gateway = lightning
            .get_gateway(None, false)
            .await
            .context("select legacy LN gateway")?;
        let description = Description::new("fi-cli payment wallet funding".to_owned())
            .context("construct legacy LN invoice description")?;
        let (operation_id, invoice, _) = lightning
            .create_bolt11_invoice(
                amount,
                Bolt11InvoiceDescriptionV1::Direct(description),
                Some(u64::from(expiry_secs)),
                serde_json::json!({ "purpose": "fi-cli-payment-wallet-funding" }),
                gateway,
            )
            .await
            .context("create legacy LN funding invoice")?;
        Ok((invoice.to_string(), operation_id))
    }

    /// Await a previously created Lightning receive operation after any restart.
    pub async fn await_invoice(
        &self,
        federation_id: FederationId,
        operation_id: OperationId,
    ) -> Result<PaymentWalletInvoiceState, WalletError> {
        let client = self.client(federation_id).await?;
        let operation = client
            .operation_log()
            .get_operation(operation_id)
            .await
            .context("Lightning receive operation not found")?;

        if operation.operation_module_kind() == LightningV2ClientModule::kind().as_str() {
            let lightning = client
                .get_first_module::<LightningV2ClientModule>()
                .context("ln-v2 module")?;
            let state = lightning
                .await_final_receive_operation_state(operation_id)
                .await
                .context("await LN-v2 funding invoice")?;
            return Ok(match state {
                LightningV2ReceiveState::Claimed => PaymentWalletInvoiceState::Claimed,
                LightningV2ReceiveState::Expired => PaymentWalletInvoiceState::Expired,
                LightningV2ReceiveState::Failure => PaymentWalletInvoiceState::Failure,
            });
        }

        if operation.operation_module_kind() == LightningV1ClientModule::kind().as_str() {
            let lightning = client
                .get_first_module::<LightningV1ClientModule>()
                .context("ln module")?;
            let mut updates = lightning
                .subscribe_ln_receive(operation_id)
                .await
                .context("subscribe to legacy LN funding invoice")?
                .into_stream();
            while let Some(state) = updates.next().await {
                match state {
                    LightningV1ReceiveState::Claimed => {
                        return Ok(PaymentWalletInvoiceState::Claimed);
                    }
                    LightningV1ReceiveState::Canceled {
                        reason: LightningV1ReceiveError::Timeout,
                    } => return Ok(PaymentWalletInvoiceState::Expired),
                    LightningV1ReceiveState::Canceled { .. } => {
                        return Ok(PaymentWalletInvoiceState::Failure);
                    }
                    LightningV1ReceiveState::Created
                    | LightningV1ReceiveState::WaitingForPayment { .. }
                    | LightningV1ReceiveState::Funded
                    | LightningV1ReceiveState::AwaitingFunds => {}
                }
            }
            return Err(anyhow::anyhow!(
                "legacy Lightning receive stream ended before a terminal state"
            )
            .into());
        }

        Err(anyhow::anyhow!(
            "operation {} is not a supported Lightning receive",
            operation_id.fmt_full()
        )
        .into())
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

    /// Reissue externally finalized mint-v2 notes into ordinary wallet
    /// balance (the tail of a refund submission).
    pub async fn receive_v2_notes(
        &self,
        federation_id: FederationId,
        mint: &MintV2ClientModule,
        notes: Vec<fedimint_mintv2_client::SpendableNote>,
    ) -> Result<Amount, WalletError> {
        let client = self.client(federation_id).await?;
        let ecash = MintV2Ecash::new(federation_id, notes);
        let amount = ecash.amount();
        let expected_operation_id = OperationId::from_encodable(&ecash);
        let operation_id = match mint.receive(ecash, serde_json::Value::Null).await {
            Ok(operation_id) => {
                if operation_id != expected_operation_id {
                    return Err(anyhow::anyhow!(
                        "mint-v2 receive returned an unexpected operation id"
                    )
                    .into());
                }
                operation_id
            }
            Err(MintV2ReceiveError::AlreadyReceived) => expected_operation_id,
            Err(error) => {
                return Err(anyhow::Error::new(error)
                    .context("start mint-v2 reissue")
                    .into());
            }
        };
        match mint
            .await_final_receive_operation_state(operation_id)
            .await
            .context("await mint-v2 reissue")?
        {
            MintV2ReceiveState::Success => {
                let operation = client
                    .operation_log()
                    .get_operation(operation_id)
                    .await
                    .context("mint-v2 receive operation is missing")?;
                let change_outpoint_range = match operation
                    .try_meta::<MintV2OperationMeta>()
                    .context("mint-v2 receive operation has incompatible metadata")?
                {
                    MintV2OperationMeta::Receive {
                        change_outpoint_range,
                        ..
                    } => change_outpoint_range,
                    MintV2OperationMeta::Send { .. } | MintV2OperationMeta::Reissue { .. } => {
                        return Err(anyhow::anyhow!(
                            "mint-v2 receive operation has the wrong metadata kind"
                        )
                        .into());
                    }
                };
                client
                    .await_primary_bitcoin_module_outputs(
                        operation_id,
                        change_outpoint_range.into_iter().collect(),
                    )
                    .await
                    .context("finalize mint-v2 received change")?;
                Ok(amount)
            }
            MintV2ReceiveState::Rejected => Err(anyhow::anyhow!("mint-v2 reissue rejected").into()),
        }
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

    /// Run the shared refund preparation under this wallet's own secrets to
    /// build the commitment sent with a signed quote request.
    pub async fn prepare_quote_refund(
        &self,
        federation_id: FederationId,
        price_msats: u64,
        refund_nonce: [u8; 32],
    ) -> Result<RefundIssuance, WalletError> {
        let client = self.client(federation_id).await?;
        refund::prepare_quote_refund(
            &client,
            &self.root_secret,
            &self.wallet_secret,
            federation_id,
            price_msats,
            refund_nonce,
        )
        .await
        .map_err(Into::into)
    }

    /// Reconstruct the mint-v1 refund secrets for one presentation (see
    /// [`refund::prepare_refund_v1`]).
    pub async fn prepare_refund_v1(
        &self,
        federation_id: FederationId,
        paid: &[locked_payment::IssuanceRequest],
        quote_id: [u8; 32],
    ) -> Result<PreparedRefund, WalletError> {
        let client = self.client(federation_id).await?;
        let mint = client
            .get_first_module::<MintClientModule>()
            .context("mint module")?;
        refund::prepare_refund_v1(&mint, &self.wallet_secret, paid, quote_id).map_err(Into::into)
    }

    /// Reconstruct the mint-v2 refund requests for one presentation (see
    /// [`refund::prepare_refund_v2`]).
    pub async fn prepare_refund_v2(
        &self,
        federation_id: FederationId,
        mint_module: ModuleInstanceId,
        paid: &[locked_payment_v2::IssuanceRequest],
        quote_id: [u8; 32],
    ) -> Result<PreparedRefundV2, WalletError> {
        let client = self.client(federation_id).await?;
        let mint = mint_v2_module(&client, mint_module)?;
        refund::prepare_refund_v2(
            mint,
            &self.root_secret,
            &self.wallet_secret,
            federation_id,
            mint_module,
            paid,
            quote_id,
        )
        .map_err(Into::into)
    }

    /// The raw Fedimint client handle behind a joined payment federation;
    /// the payer protocol in [`crate::payer`] runs directly against it.
    pub async fn client(
        &self,
        federation_id: FederationId,
    ) -> Result<ClientHandleArc, WalletError> {
        self.federations
            .read()
            .await
            .get(&federation_id)
            .cloned()
            .ok_or(WalletError::UnknownFederation(federation_id))
    }
}

struct OobReissueTag;
impl sha256t::Tag for OobReissueTag {
    fn engine() -> sha256::HashEngine {
        let mut engine = sha256::HashEngine::default();
        engine.input(b"oob-reissue");
        engine
    }
}

/// The deterministic reissue operation id fedimint-mint-client assigns a
/// note set, reproduced so an interrupted receive can resume it.
fn reissue_operation_id(notes: &OOBNotes) -> OperationId {
    OperationId(
        notes
            .notes()
            .consensus_hash::<sha256t::Hash<OobReissueTag>>()
            .to_byte_array(),
    )
}

async fn client_builder() -> anyhow::Result<fedimint_client::ClientBuilder> {
    let mut builder = Client::builder().await?;
    builder.with_module(MintClientInit);
    builder.with_module(MintV2ClientInit);
    builder.with_module(WalletV1ClientInit::default());
    builder.with_module(WalletV2ClientInit);
    builder.with_module(LightningV1ClientInit::default());
    builder.with_module(LightningV2ClientInit::default());
    builder.with_module(StabilityPoolClientInit::default());
    Ok(builder)
}

async fn connectors() -> anyhow::Result<ConnectorRegistry> {
    ConnectorRegistry::build_from_client_defaults().bind().await
}
