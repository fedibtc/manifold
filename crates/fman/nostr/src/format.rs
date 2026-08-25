//! How backup data is stored on relays
//! (`crates/fman/specs/SPEC-nostr-backup-restore.md`, *Documents*).
//!
//! `fman-core` owns *what* is worth backing up — the seat document's contents
//! and the guardian archive's bytes ([`fman_core::backup`]). This module owns
//! everything that exists only because the storage is Nostr: the sealed
//! payloads, the blinded coordinates events live at, the slicing that fits a
//! hundred-kilobyte archive into events, the fixed-size padding, and the
//! schema version a reader must know before it can read anything.
//!
//! **Sealing is XChaCha20-Poly1305 under a dedicated mnemonic-derived key**
//! ([`fman_core::identity::RootMnemonic::derive_nostr_backup_encryption_key`]),
//! the same shape as fedimint's own client backup. A payload is CBOR — it is
//! carried and sealed as bytes, so a binary encoding is the natural fit —
//! framed with its length, and sealed *whole*: the seat document as one
//! padded plaintext, the guardian archive as one blob whose ciphertext is
//! then sliced across events. The slices carry no structure — no index, no
//! count, no digest — because the blinded coordinate already addresses them
//! in order, the AEAD tag already authenticates the reassembled whole, and
//! the seat's own document already names the digest that binds the archive
//! to it.
//!
//! Two properties this module is responsible for:
//!
//! - **Guardian config is opaque.** `fedimintd`'s config files are carried and
//!   restored byte for byte. Nothing here parses or re-encrypts them, so the
//!   backup format is not coupled to a Fedimint server-config file format;
//!   the native API client does not parse or reconstruct that archive
//!   (ARCH-fleet-manager, `fedimint_api`).
//! - **Every event is the same size.** Seat documents are padded to a fixed
//!   plaintext length, and the archive's padding is chosen so its ciphertext
//!   slices come out at exactly a sealed document's length — an event's
//!   length discloses nothing about the seat behind it. Seat *count* and
//!   publication timing remain observable and are accepted disclosures.

use std::collections::HashMap;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use chacha20poly1305::aead::{Aead as _, AeadCore as _, KeyInit as _, OsRng, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use fedi_decentralized_service_fleet_manager::SeatId;
use fman_core::backup::{
    GuardianArchive, RecoverError, RecoveredFleet, SeatBackupDocument, SeatPublication, sha256_hex,
};
use nostr_sdk::{Event, EventBuilder, Keys, Kind, Tag};
use serde::{Deserialize, Serialize};

/// The sealed frame: `u32-le payload length ‖ CBOR payload ‖ zero padding`.
/// Explicit because the payload is binary — there is no textual trick like
/// trailing whitespace to find its end.
const LEN_PREFIX: usize = 4;

/// Associated data separating the two seal domains. Classification on
/// restore is "does it open as a document?", and without domain separation
/// an archive small enough to seal into a *single* slice would open there
/// too — a complete AEAD whole that then fails to parse, wrongly reported
/// as an unreadable document instead of claimed as an archive. The AAD
/// makes the family part of the cryptographic statement: a payload opens
/// under exactly one domain, whatever its size.
const AAD_DOCUMENT: &[u8] = b"fman-backup/document";
const AAD_ARCHIVE: &[u8] = b"fman-backup/archive";

/// Payload capacity of a seat document: one event's plaintext minus the
/// frame's length prefix. The one constant both the seal-time check and its
/// error message quote.
const DOCUMENT_CAPACITY: usize = PADDED_PLAINTEXT_LEN - LEN_PREFIX;

/// Plaintext capacity of one event ([`BackupIdentity::seal_padded`] pads
/// every payload to a whole number of these), and so the hard cap on a seat
/// document, which must stay a single event. The frame's length prefix
/// ([`LEN_PREFIX`]) spends four of these bytes.
///
/// Sized generously for a document that is a few hundred bytes of facts; a
/// document that does not fit is a hard error, never a silently truncated
/// backup.
const PADDED_PLAINTEXT_LEN: usize = 32 * 1024;

/// XChaCha20-Poly1305 framing: a random nonce prefixed to the ciphertext, and
/// the authentication tag the cipher appends.
const NONCE_LEN: usize = 24;
const TAG_LEN: usize = 16;
const SEAL_OVERHEAD: usize = NONCE_LEN + TAG_LEN;

/// Every event's content length before base64: a sealed document, or one
/// slice of a sealed archive (whose padding is chosen to make that true).
const SEALED_LEN: usize = PADDED_PLAINTEXT_LEN + SEAL_OVERHEAD;

/// Current document schema version. Restore refuses versions it does not know
/// rather than guessing at a partially understood recovery payload.
///
/// One version for the whole format, carried on the seat document's envelope
/// ([`Envelope`]) and nowhere else: a schema version describes what the
/// reader must understand to read *anything*, and every archive is reached
/// through some seat document, so a second copy would be a number that must
/// agree with it for no one's benefit.
///
/// Confirmed-publication records are scoped to this value
/// ([`fman_core::backup::BackupSink::format_version`]): a record written under
/// another version describes events this build's own reader refuses, so
/// bumping it republishes every seat.
pub(crate) const BACKUP_DOCUMENT_VERSION: u32 = 1;

/// Failure building or reading a backup payload.
#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("serialize backup document: {0}")]
    Serialize(#[source] ciborium::ser::Error<std::io::Error>),
    #[error("backup document is {len} bytes, over the {DOCUMENT_CAPACITY}-byte document limit")]
    TooLarge { len: usize },
    #[error("backup payload does not open under this mnemonic's backup key")]
    Decrypt,
    #[error("backup payload frame is malformed")]
    Frame,
    #[error("parse backup document: {0}")]
    Parse(#[source] ciborium::de::Error<std::io::Error>),
    #[error(
        "backup document version {found} is not supported (expected {BACKUP_DOCUMENT_VERSION})"
    )]
    UnsupportedVersion { found: u32 },
    #[error(
        "guardian archive does not match the digest the backup names (expected {expected}, got {found})"
    )]
    ArchiveMismatch { expected: String, found: String },
}

/// What a seat-document event seals: the document and the version needed to
/// read it. A plain nested field rather than serde(flatten): nobody reads
/// the encoded form, and a self-contained field keeps the version readable
/// on its own.
#[derive(Serialize, Deserialize)]
struct Envelope<T> {
    version: u32,
    document: T,
}

/// The identity that authors, seals, and *addresses* backup events.
///
/// Three keys, all from the root mnemonic and none shared with the service
/// identity: one signs events, one seals their contents, one blinds their
/// coordinates. They are carried together because a document is unpublishable
/// without all of them, and separated by derivation so no key's exposure
/// yields another's job.
#[derive(Clone)]
pub struct BackupIdentity {
    keys: Keys,
    tag_key: [u8; 32],
    cipher: XChaCha20Poly1305,
}

impl BackupIdentity {
    pub fn derive(identity: &fman_core::identity::RootMnemonic) -> Self {
        Self {
            keys: identity.derive_nostr_backup_keys(),
            tag_key: identity.derive_nostr_backup_tag_key(),
            cipher: XChaCha20Poly1305::new((&identity.derive_nostr_backup_encryption_key()).into()),
        }
    }

    pub fn keys(&self) -> &Keys {
        &self.keys
    }

    /// The events one publication becomes, in publication order: archive
    /// slices first, the seat's own document last, so a seat document on a
    /// relay is never the newer half of a pair — it names a digest, and a
    /// digest verifies an archive rather than conjuring one. A failure
    /// partway leaves at worst confirmed slices under an old document, and
    /// rewriting immutable bytes at the same coordinates on the retry is the
    /// cheap side of that.
    pub fn publication_events(
        &self,
        publication: &SeatPublication,
    ) -> Result<Vec<EventBuilder>, BackupError> {
        let seat_id = &publication.document.seat_id;
        let mut events = Vec::new();
        if let Some(archive) = &publication.archive {
            for (index, content) in self.seal_archive(archive).into_iter().enumerate() {
                let index = u32::try_from(index).expect("an archive fits u32 slices");
                events.push(addressable_event(
                    content,
                    self.archive_coordinate(seat_id, index),
                ));
            }
        }
        events.push(addressable_event(
            self.seal_seat_document(&publication.document)?,
            self.seat_coordinate(seat_id),
        ));
        Ok(events)
    }

    /// A document must stay a *single* event: it is the standalone-openable
    /// payload a restore bootstraps seat ids from, and nothing else opens in
    /// its domain — a slice of a larger seal does not verify on its own, and
    /// a whole archive seal carries [`AAD_ARCHIVE`] (a Poly1305 false
    /// authentication is a ~2^-128 event, not an impossibility). So a
    /// document that would need a second slice is a hard error, never a
    /// silently spanned one.
    fn seal_seat_document(&self, document: &SeatBackupDocument) -> Result<String, BackupError> {
        let mut payload = Vec::new();
        ciborium::into_writer(
            &Envelope {
                version: BACKUP_DOCUMENT_VERSION,
                document,
            },
            &mut payload,
        )
        .map_err(BackupError::Serialize)?;
        if payload.len() > DOCUMENT_CAPACITY {
            return Err(BackupError::TooLarge { len: payload.len() });
        }
        let mut slices = self.seal_padded(payload, AAD_DOCUMENT);
        debug_assert_eq!(
            slices.len(),
            1,
            "a document fits one slice by the check above"
        );
        Ok(slices.remove(0))
    }

    /// Open and parse a document sealed by [`Self::seal_seat_document`],
    /// refusing any schema version this build does not know.
    ///
    /// The version check lives here rather than in the caller because every
    /// reader of a document is a restore path acting on recovery material:
    /// guessing at a partially understood payload is the one outcome none of
    /// them may have, so the refusal is not theirs to forget.
    pub(crate) fn open_seat_document(
        &self,
        content: &str,
    ) -> Result<SeatBackupDocument, BackupError> {
        let blob = BASE64.decode(content).map_err(|_| BackupError::Decrypt)?;
        let plaintext = self.open_blob(&blob, AAD_DOCUMENT)?;
        let payload = unframe(&plaintext)?;
        // The version is read before the body, so a document from a schema
        // this build does not know is refused rather than parsed into
        // whichever fields happen to still match.
        let version: Envelope<serde::de::IgnoredAny> =
            ciborium::from_reader(payload).map_err(BackupError::Parse)?;
        if version.version != BACKUP_DOCUMENT_VERSION {
            return Err(BackupError::UnsupportedVersion {
                found: version.version,
            });
        }
        // The declared payload must be exactly one CBOR item: bytes hiding
        // after it would be a second spelling of the same document, and this
        // frame has one.
        let mut cursor = std::io::Cursor::new(payload);
        let envelope: Envelope<SeatBackupDocument> =
            ciborium::from_reader(&mut cursor).map_err(BackupError::Parse)?;
        if cursor.position() as usize != payload.len() {
            return Err(BackupError::Frame);
        }
        Ok(envelope.document)
    }

    /// The base64 event contents one seat's guardian archive is sealed into.
    ///
    /// The archive runs to a hundred kilobytes or more — `consensus.json`
    /// alone holds a BLS public key share per mint denomination tier per
    /// guardian — so it does not fit one event; [`Self::seal_padded`] slices
    /// it across however many it needs.
    fn seal_archive(&self, archive: &GuardianArchive) -> Vec<String> {
        self.seal_padded(archive.to_bytes(), AAD_ARCHIVE)
    }

    /// The one framing-padding-sealing formula every payload goes through:
    /// prefix the payload with its length, zero-pad to the smallest whole
    /// number of events, seal as one AEAD blob, slice the blob at
    /// [`SEALED_LEN`], base64 each slice. Padding to whole events is what
    /// makes every event on the relay one length; a seat document is simply
    /// the one-event case.
    fn seal_padded(&self, payload: Vec<u8>, aad: &[u8]) -> Vec<String> {
        let mut plaintext = Vec::with_capacity(LEN_PREFIX + payload.len());
        plaintext.extend_from_slice(
            &u32::try_from(payload.len())
                .expect("a backup payload fits u32")
                .to_le_bytes(),
        );
        plaintext.extend_from_slice(&payload);
        let events = (plaintext.len() + SEAL_OVERHEAD).div_ceil(SEALED_LEN);
        plaintext.resize(events * SEALED_LEN - SEAL_OVERHEAD, 0);
        let blob = self.seal_blob(&plaintext, aad);
        blob.chunks(SEALED_LEN)
            .map(|slice| BASE64.encode(slice))
            .collect()
    }

    /// Reassemble and open one seat's archive from its slices, in coordinate
    /// order.
    ///
    /// The AEAD tag is the integrity check — a missing, reordered, foreign,
    /// or tampered slice makes the whole blob refuse to open — and the digest
    /// from the seat's own document is the binding check on top of it: the
    /// slices demonstrably carry *an* archive this mnemonic sealed, the
    /// digest says it is *this seat's*.
    fn open_archive(
        &self,
        slices: &[Vec<u8>],
        expected_digest: &str,
    ) -> Result<GuardianArchive, BackupError> {
        let plaintext = self.open_blob(&slices.concat(), AAD_ARCHIVE)?;
        let payload = unframe(&plaintext)?;
        let found = sha256_hex(payload);
        if found != expected_digest {
            return Err(BackupError::ArchiveMismatch {
                expected: expected_digest.to_owned(),
                found,
            });
        }
        let mut cursor = std::io::Cursor::new(payload);
        let archive = ciborium::from_reader(&mut cursor).map_err(BackupError::Parse)?;
        if cursor.position() as usize != payload.len() {
            return Err(BackupError::Frame);
        }
        Ok(archive)
    }

    /// Seal a plaintext into `nonce ‖ ciphertext ‖ tag`, [`SEAL_OVERHEAD`]
    /// bytes longer than what went in. The `aad` names the seal's domain
    /// ([`AAD_DOCUMENT`]/[`AAD_ARCHIVE`]) and is bound into the tag.
    fn seal_blob(&self, plaintext: &[u8], aad: &[u8]) -> Vec<u8> {
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        let ciphertext = self
            .cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .expect("XChaCha20-Poly1305 seals any buffer");
        let mut blob = Vec::with_capacity(NONCE_LEN + ciphertext.len());
        blob.extend_from_slice(&nonce);
        blob.extend_from_slice(&ciphertext);
        blob
    }

    fn open_blob(&self, blob: &[u8], aad: &[u8]) -> Result<Vec<u8>, BackupError> {
        if blob.len() < SEAL_OVERHEAD {
            return Err(BackupError::Decrypt);
        }
        let (nonce, ciphertext) = blob.split_at(NONCE_LEN);
        self.cipher
            .decrypt(
                XNonce::from_slice(nonce),
                Payload {
                    msg: ciphertext,
                    aad,
                },
            )
            .map_err(|_| BackupError::Decrypt)
    }

    /// The blinded `d` tag one seat's document lives at.
    ///
    /// A seat id is the canonical hex of a quote id, which the FI that bought
    /// the seat holds: publishing it in the clear would let that FI query the
    /// relays for its own seat and resolve the FMan's backup identity, which is
    /// exactly the link the separate derivation exists to break. So the
    /// coordinate is `HMAC(tag_key, "seat:" || seat_id)`: stable, unique per
    /// seat, and unguessable without the phrase.
    ///
    /// The document family is inside the MAC rather than a plaintext prefix,
    /// so the archive family neither collides with this one nor announces
    /// itself on the relay.
    fn seat_coordinate(&self, seat_id: &SeatId) -> String {
        self.coordinate(b"seat:", seat_id.to_string().as_bytes())
    }

    /// Where one slice of a seat's guardian archive lives, beside the seat's
    /// own document. The index is inside the MAC too: it is the *only* record
    /// of slice order, and it makes each slice's address unique.
    fn archive_coordinate(&self, seat_id: &SeatId, index: u32) -> String {
        self.coordinate(b"archive:", format!("{seat_id}:{index}").as_bytes())
    }

    fn coordinate(&self, family: &[u8], value: &[u8]) -> String {
        use hmac::Mac as _;
        let mut mac = <hmac::Hmac<sha2::Sha256> as hmac::Mac>::new_from_slice(&self.tag_key)
            .expect("HMAC accepts a 32-byte key");
        mac.update(family);
        mac.update(value);
        hex::encode(mac.finalize().into_bytes())
    }
}

/// Backup events carry no hashtag and no identifying tags beyond their blinded
/// addressable coordinate: the `d` tag is the only thing addressability needs,
/// and anything more would describe in the clear what the content seals.
fn addressable_event(content: String, d_tag: String) -> EventBuilder {
    EventBuilder::new(
        Kind::Custom(fedi_decentralized_nostr::fman::FMAN_BACKUP_EVENT_KIND),
        content,
    )
    .tags([Tag::parse(["d", &d_tag]).expect("valid FMan backup d tag")])
}

/// The payload of a sealed frame: read the length prefix, return exactly
/// those bytes. The frame sits inside a verified seal, so a malformed one is
/// our own writing gone wrong — fatal, not skippable. Canonical means
/// canonical: the padding must be all zeros and less than one event of it
/// (i.e. the writer used the fewest events the payload allows), so there is
/// exactly one sealed spelling of a payload for a reader to accept.
fn unframe(plaintext: &[u8]) -> Result<&[u8], BackupError> {
    let (len, rest) = plaintext
        .split_at_checked(LEN_PREFIX)
        .ok_or(BackupError::Frame)?;
    let len = u32::from_le_bytes(len.try_into().expect("split at LEN_PREFIX")) as usize;
    let (payload, padding) = rest.split_at_checked(len).ok_or(BackupError::Frame)?;
    if padding.len() >= SEALED_LEN || padding.iter().any(|&byte| byte != 0) {
        return Err(BackupError::Frame);
    }
    Ok(payload)
}

/// The half of a restore that has no relay in it: given the events, what
/// fleet do they describe?
///
/// Classification is by the cipher, not by plaintext markers: a content that
/// opens under the document domain is a seat document, and one that does not
/// is a candidate slice of an archive ciphertext — a slice of a larger seal
/// does not verify on its own, and even an archive small enough to seal into
/// a single slice opens only under [`AAD_ARCHIVE`] (a Poly1305 false
/// authentication is a ~2^-128 event). Each formed seat then claims its slices by computing
/// their coordinates — index 0 upward until a coordinate has no event — and
/// opening the reassembled blob, so a slice set that is incomplete, reordered,
/// or another seat's refuses as one AEAD failure.
///
/// **An unreadable or unclaimed event is fatal.** These events are
/// signature-verified and filtered by author, so an event that arrives here
/// was published by this mnemonic: a document that opens but will not parse,
/// and a slice no seat claims, are both *ours* and unaccounted for. Skipping
/// either would restore a fleet missing whatever it held, once and for good.
pub fn recover_from_events(
    backup: &BackupIdentity,
    events: Vec<Event>,
) -> Result<RecoveredFleet, RecoverError> {
    let mut seats = Vec::new();
    let mut claimed_seats = std::collections::HashSet::new();
    let mut slices: HashMap<String, Event> = HashMap::new();
    for event in events {
        match backup.open_seat_document(&event.content) {
            Ok(seat) => {
                // The document names its own coordinate (it is derived from
                // the seat id inside it), so a document at any other address
                // — or a second document at the same one — is an enumeration
                // that cannot be trusted to be the latest of anything.
                let coordinate = backup.seat_coordinate(&seat.seat_id);
                if event.tags.identifier() != Some(coordinate.as_str()) {
                    return Err(RecoverError::UnreadableDocument(event.id.to_string()));
                }
                if !claimed_seats.insert(coordinate) {
                    return Err(anyhow::anyhow!(
                        "the relay served two backup events at one coordinate"
                    )
                    .into());
                }
                seats.push(seat);
            }
            Err(BackupError::Decrypt) => {
                let coordinate = event.tags.identifier().unwrap_or_default().to_owned();
                if slices.insert(coordinate, event).is_some() {
                    // Addressable events replace: the relay serving two events
                    // at one coordinate means the enumeration cannot be
                    // trusted to be the latest of anything.
                    return Err(anyhow::anyhow!(
                        "the relay served two backup events at one coordinate"
                    )
                    .into());
                }
            }
            Err(_) => return Err(RecoverError::UnreadableDocument(event.id.to_string())),
        }
    }

    let mut archives = HashMap::new();
    for seat in &seats {
        let Some(guardian) = &seat.guardian else {
            continue;
        };
        let mut found = Vec::new();
        for index in 0.. {
            match slices.remove(&backup.archive_coordinate(&seat.seat_id, index)) {
                Some(event) => found.push(
                    BASE64
                        .decode(&event.content)
                        .map_err(|_| RecoverError::UnreadableDocument(event.id.to_string()))?,
                ),
                None => break,
            }
        }
        if found.is_empty() {
            // No archive on the relays: the install refuses this seat with
            // its own MissingArchive error, which names the seat.
            continue;
        }
        let archive = backup
            .open_archive(&found, &guardian.archive_sha256)
            .map_err(|err| anyhow::anyhow!("guardian archive for seat {}: {err}", seat.seat_id))?;
        archives.insert(seat.seat_id.clone(), archive);
    }

    if let Some(event) = slices.into_values().min_by_key(|event| event.id) {
        return Err(RecoverError::UnreadableDocument(event.id.to_string()));
    }

    Ok(RecoveredFleet {
        seats,
        archives,
        format_version: BACKUP_DOCUMENT_VERSION,
    })
}

#[cfg(test)]
#[path = "format_tests.rs"]
mod tests;
