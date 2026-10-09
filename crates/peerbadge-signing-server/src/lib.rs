//! Single-use blind issuance with bounded volatile state and durable audit commits.

mod audit;
pub mod issuer;

use std::{
    collections::{HashMap, VecDeque},
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::ensure;
use fedi_decentralized_service_peerbadge_signing::*;
use fedi_iroh_rpc::IrohProtocol;
use hmac::{Hmac, Mac as _};
use iroh::{Endpoint, protocol::Router};
use nostr_sdk::{
    PublicKey,
    secp256k1::{Message, schnorr::Signature},
};
use peerbadge_protocol::IssuanceRequest;
use rand::RngCore as _;
use serde::Deserialize;
use sha2::{Digest as _, Sha256};

use audit::{Audit, AuditEvent, unavailable};
use issuer::IssuerMaterial;

const CHALLENGE_TTL: u64 = 60;
const SESSION_TTL: u64 = 600;
const TERMINAL_RETENTION: u64 = 600;
const MAX_ENTRIES: usize = 4096;
const MAX_REQUEST_BYTES: usize = 64 * 1024;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Signer {
    pub pubkey: String,
    pub max_level: u8,
    pub max_sessions_per_hour: u32,
}

struct SignerState {
    config: Signer,
    admissions: VecDeque<u64>,
}

struct Session {
    signer: String,
    level: u8,
    expires_at: u64,
    state: SessionState,
    redemption: Option<CachedRedemption>,
}

struct CachedRedemption {
    request_sha256: String,
    response: String,
}

struct State {
    issuer: IssuerMaterial,
    issuer_id: [u8; 32],
    signers: HashMap<PublicKey, SignerState>,
    nonce_key: [u8; 32],
    consumed_nonces: HashMap<[u8; 32], u64>,
    sessions: HashMap<String, Session>,
    audit: Audit,
}

/// Clones share admission, consumption, and audit state atomically.
#[derive(Clone)]
pub struct SigningServer {
    state: Arc<Mutex<State>>,
}

impl SigningServer {
    pub fn new(
        issuer: IssuerMaterial,
        signers: Vec<Signer>,
        audit_path: &Path,
    ) -> anyhow::Result<Self> {
        ensure!(signers.len() <= MAX_ENTRIES, "too many signers");
        let mut allowlist = HashMap::new();
        for mut signer in signers {
            ensure!(
                (1..=9).contains(&signer.max_level),
                "invalid signer level cap"
            );
            ensure!(
                signer.max_sessions_per_hour > 0,
                "signer rate must be positive"
            );
            let pubkey = parse_pubkey(&signer.pubkey)
                .ok_or_else(|| anyhow::anyhow!("invalid signer public key"))?;
            signer.pubkey = pubkey.to_hex();
            ensure!(
                allowlist
                    .insert(
                        pubkey,
                        SignerState {
                            config: signer,
                            admissions: VecDeque::new()
                        }
                    )
                    .is_none(),
                "duplicate signer"
            );
        }
        let issuer_id = issuer.authority.issuer.issuer_id_pubkey.0.to_bytes();
        Ok(Self {
            state: Arc::new(Mutex::new(State {
                issuer,
                issuer_id,
                signers: allowlist,
                nonce_key: random_bytes(),
                consumed_nonces: HashMap::new(),
                sessions: HashMap::new(),
                audit: Audit::open(audit_path)?,
            })),
        })
    }

    // RSA and durable file writes must not block a Tokio worker. The protocol's
    // handler limit bounds blocking jobs; the lock also prevents double redeem.
    async fn run<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut State, u64) -> Result<T, SigningError> + Send + 'static,
    ) -> Result<T, SigningError> {
        let state = self.state.clone();
        tokio::task::spawn_blocking(move || {
            let mut state = state.lock().map_err(|_| unavailable())?;
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| unavailable())?
                .as_secs();
            state.sweep(now)?;
            operation(&mut state, now)
        })
        .await
        .map_err(|_| unavailable())?
    }

    /// Expire unused capabilities even when no clients are making calls.
    pub async fn sweep(&self) -> Result<(), SigningError> {
        self.run(|_, _| Ok(())).await
    }
}

/// The same bounded transport is used by the CLI and native integration tests.
pub fn spawn_router(endpoint: Endpoint, server: SigningServer) -> Router {
    Router::builder(endpoint)
        .accept(
            PEERBADGE_SIGNING_ALPN,
            IrohProtocol::with_limits_and_request_read_timeout(
                PeerBadgeSigningServiceServer::new(server),
                MAX_REQUEST_BYTES,
                32,
                Duration::from_secs(5),
            ),
        )
        .spawn()
}

impl PeerBadgeSigningService for SigningServer {
    async fn challenge(&self, _req: ChallengeRequest) -> Result<ChallengeResponse, SigningError> {
        self.run(|state, now| state.challenge(now)).await
    }

    async fn open_session(
        &self,
        req: OpenSessionRequest,
    ) -> Result<OpenSessionResponse, SigningError> {
        self.run(move |state, now| state.open(req, now)).await
    }

    async fn redeem_session(
        &self,
        req: RedeemSessionRequest,
    ) -> Result<RedeemSessionResponse, SigningError> {
        self.run(move |state, now| state.redeem(req, now)).await
    }

    async fn session_status(
        &self,
        req: SessionStatusRequest,
    ) -> Result<SessionStatusResponse, SigningError> {
        self.run(move |state, _| {
            let session = state
                .sessions
                .get(&req.session_id)
                .ok_or(SigningError::SessionNotFound)?;
            Ok(SessionStatusResponse {
                state: session.state,
            })
        })
        .await
    }
}

impl State {
    fn sweep(&mut self, now: u64) -> Result<(), SigningError> {
        self.audit.check()?;
        self.consumed_nonces
            .retain(|_, expires_at| now < *expires_at);
        for (id, session) in &mut self.sessions {
            if now >= session.expires_at {
                session.redemption = None;
            }
            if now >= session.expires_at && matches!(session.state, SessionState::Open) {
                self.audit.record(AuditEvent {
                    ts: now,
                    event: "session_expired",
                    signer_pubkey: Some(&session.signer),
                    level: Some(session.level),
                    session_id: Some(id),
                    request_sha256: None,
                    reason: None,
                })?;
                session.state = SessionState::Expired;
            }
        }
        self.sessions
            .retain(|_, session| now < session.expires_at.saturating_add(TERMINAL_RETENTION));
        for signer in self.signers.values_mut() {
            while signer
                .admissions
                .front()
                .is_some_and(|ts| now >= ts.saturating_add(3600))
            {
                signer.admissions.pop_front();
            }
        }
        Ok(())
    }

    fn challenge(&self, now: u64) -> Result<ChallengeResponse, SigningError> {
        let expires_at = now + CHALLENGE_TTL;
        let mut nonce = [0; 32];
        rand::rngs::OsRng.fill_bytes(&mut nonce[..16]);
        nonce[16..24].copy_from_slice(&expires_at.to_be_bytes());
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&self.nonce_key).expect("HMAC accepts a 32-byte key");
        mac.update(&nonce[..24]);
        nonce[24..].copy_from_slice(&mac.finalize().into_bytes()[..8]);
        Ok(ChallengeResponse {
            nonce: nonce.to_vec(),
            expires_at,
            issuer_id_pubkey: hex::encode(self.issuer_id),
        })
    }

    fn authenticate_nonce(&self, bytes: &[u8]) -> Result<([u8; 32], u64), SigningError> {
        let nonce: [u8; 32] = bytes.try_into().map_err(|_| SigningError::BadChallenge)?;
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&self.nonce_key).expect("HMAC accepts a 32-byte key");
        mac.update(&nonce[..24]);
        mac.verify_truncated_left(&nonce[24..])
            .map_err(|_| SigningError::BadChallenge)?;
        let expires_at = u64::from_be_bytes(nonce[16..24].try_into().expect("8-byte expiry"));
        Ok((nonce, expires_at))
    }

    fn open(
        &mut self,
        req: OpenSessionRequest,
        now: u64,
    ) -> Result<OpenSessionResponse, SigningError> {
        // Unauthenticated challenge traffic must not consume state or audit space.
        let (nonce, expires_at) = self.authenticate_nonce(&req.nonce)?;
        let pubkey = parse_pubkey(&req.signer_pubkey);
        // Store only canonical public keys, never arbitrary attacker text.
        let signer_hex = pubkey.map(|key| key.to_hex());
        let result = self.authorize(&req, pubkey, nonce, expires_at, now);
        let pubkey = match result {
            Ok(pubkey) => pubkey,
            Err(error) => {
                let reason = match &error {
                    SigningError::BadChallenge => "bad_challenge",
                    SigningError::LevelNotAllowed { .. } => "level_not_allowed",
                    SigningError::RateLimited => "rate_limited",
                    _ => "unauthorized",
                };
                self.audit.record(AuditEvent {
                    ts: now,
                    event: "open_rejected",
                    signer_pubkey: signer_hex.as_deref(),
                    level: Some(req.level),
                    session_id: None,
                    request_sha256: None,
                    reason: Some(reason),
                })?;
                return Err(error);
            }
        };
        let session_id = loop {
            let id = hex::encode(random_bytes());
            if !self.sessions.contains_key(&id) {
                break id;
            }
        };
        let signer = signer_hex.expect("authorized key is valid");
        self.audit.record(AuditEvent {
            ts: now,
            event: "session_opened",
            signer_pubkey: Some(&signer),
            level: Some(req.level),
            session_id: Some(&session_id),
            request_sha256: None,
            reason: None,
        })?;
        let expires_at = now + SESSION_TTL;
        self.sessions.insert(
            session_id.clone(),
            Session {
                signer,
                level: req.level,
                expires_at,
                state: SessionState::Open,
                redemption: None,
            },
        );
        self.signers
            .get_mut(&pubkey)
            .expect("authorized signer")
            .admissions
            .push_back(now);
        Ok(OpenSessionResponse {
            session_id,
            info: trust_info(req.level),
            issuer_authority: self.issuer.authority_json.clone(),
            expires_at,
        })
    }

    fn authorize(
        &mut self,
        req: &OpenSessionRequest,
        pubkey: Option<PublicKey>,
        nonce: [u8; 32],
        expires_at: u64,
        now: u64,
    ) -> Result<PublicKey, SigningError> {
        if now >= expires_at || self.consumed_nonces.contains_key(&nonce) {
            return Err(SigningError::BadChallenge);
        }
        if self.consumed_nonces.len() >= MAX_ENTRIES {
            return Err(SigningError::RateLimited);
        }
        // Only authentic, live nonces consume state. Every use is a single
        // attempt, including authorization failures; never evict live protection.
        self.consumed_nonces.insert(nonce, expires_at);
        let pubkey = pubkey.ok_or(SigningError::Unauthorized)?;
        let signer = self
            .signers
            .get(&pubkey)
            .ok_or(SigningError::Unauthorized)?;
        let signature =
            Signature::from_slice(&req.signature).map_err(|_| SigningError::Unauthorized)?;
        let message = Message::from_digest(challenge_digest(&nonce, req.level, &self.issuer_id));
        nostr_sdk::SECP256K1
            .verify_schnorr(
                &signature,
                &message,
                &pubkey.xonly().map_err(|_| SigningError::Unauthorized)?,
            )
            .map_err(|_| SigningError::Unauthorized)?;
        if req.level == 0 || req.level > signer.config.max_level {
            return Err(SigningError::LevelNotAllowed {
                max: signer.config.max_level,
            });
        }
        if signer.admissions.len() >= signer.config.max_sessions_per_hour as usize
            || signer.admissions.len() >= MAX_ENTRIES
            || self.sessions.len() >= MAX_ENTRIES
        {
            return Err(SigningError::RateLimited);
        }
        Ok(pubkey)
    }

    fn redeem(
        &mut self,
        req: RedeemSessionRequest,
        now: u64,
    ) -> Result<RedeemSessionResponse, SigningError> {
        let session = self
            .sessions
            .get_mut(&req.session_id)
            .ok_or(SigningError::SessionNotFound)?;
        if now >= session.expires_at {
            session.redemption = None;
            return Err(SigningError::SessionExpired);
        }
        if matches!(session.state, SessionState::Expired) {
            return Err(SigningError::SessionExpired);
        }
        if req.request.len() > MAX_REQUEST_BYTES {
            return Err(if matches!(session.state, SessionState::Redeemed) {
                SigningError::SessionAlreadyRedeemed
            } else {
                invalid_request()
            });
        }
        let digest = hex::encode(Sha256::digest(req.request.as_bytes()));
        if matches!(session.state, SessionState::Redeemed) {
            let cached = session
                .redemption
                .as_ref()
                .filter(|cached| cached.request_sha256 == digest)
                .ok_or(SigningError::SessionAlreadyRedeemed)?;
            self.audit.record(AuditEvent {
                ts: now,
                event: "session_redeem_replayed",
                signer_pubkey: Some(&session.signer),
                level: Some(session.level),
                session_id: Some(&req.session_id),
                request_sha256: Some(&cached.request_sha256),
                reason: None,
            })?;
            return Ok(RedeemSessionResponse {
                response: cached.response.clone(),
            });
        }
        let request: IssuanceRequest =
            serde_json::from_str(&req.request).map_err(|_| invalid_request())?;
        let response = self
            .issuer
            .context
            .issue_credential(trust_info(session.level), &request)
            .map_err(|_| invalid_request())?;
        let response = serde_json::to_string(&response).map_err(|_| unavailable())?;
        // No signature escapes before its durable audit record. A failed audit
        // poisons the service, so that capability can never be signed again.
        self.audit.record(AuditEvent {
            ts: now,
            event: "session_redeemed",
            signer_pubkey: Some(&session.signer),
            level: Some(session.level),
            session_id: Some(&req.session_id),
            request_sha256: Some(&digest),
            reason: None,
        })?;
        session.state = SessionState::Redeemed;
        session.redemption = Some(CachedRedemption {
            request_sha256: digest,
            response: response.clone(),
        });
        Ok(RedeemSessionResponse { response })
    }
}

fn parse_pubkey(value: &str) -> Option<PublicKey> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    PublicKey::from_hex(value).ok()
}

fn random_bytes() -> [u8; 32] {
    let mut bytes = [0; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes
}

fn trust_info(level: u8) -> serde_json::Value {
    peerbadge_schemas::trust_score_info_v1(u64::from(level)).expect("admitted trust level is valid")
}

fn invalid_request() -> SigningError {
    SigningError::InvalidRequest("invalid issuance request".to_owned())
}

#[cfg(test)]
mod tests;
