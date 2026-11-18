//! Which MGs this FA skips in selection after they failed a key ceremony.
//!
//! An MG that passes the live availability probe can still fail the
//! ceremony it sold a seat for. The FA remembers such an MG for
//! [`CEREMONY_FAILURE_SKIP`] and does not select it meanwhile
//! ([ARCH-fa-client-discovery-selection](../specs/ARCH-fa-client-discovery-selection.md),
//! *Ceremony failures*). Only a failure clearly the MG's own counts: its seat
//! did not answer while most of the formation's seats did. A ceremony every
//! seat answers for but that does not finish, such as guardians retrying out
//! of step or running incompatible builds, is no single MG's fault.

use std::time::Duration;

use nostr_sdk::PublicKey;

use crate::{FaError, MgCause, MgFailure};

/// How long this FA skips an MG after it last failed a ceremony: one day,
/// the shortest window of the MG's own uptime gate.
pub(crate) const CEREMONY_FAILURE_SKIP: Duration = Duration::from_secs(24 * 60 * 60);

/// Whether `error` says the seat's MG, or its seat, did not answer: the
/// connection or request failed or timed out, or the MG has no running
/// guardian process for the seat. Any other answer, including a refusal,
/// shows the MG was there.
pub(crate) fn is_absence(error: &FaError) -> bool {
    matches!(
        error,
        FaError::Mg(MgFailure {
            cause: MgCause::Timeout | MgCause::Transport | MgCause::SeatUnavailable,
            ..
        })
    )
}

/// The MGs one ceremony round attributes a failure to, from each seat's MG
/// and whether it was absent ([`is_absence`]). The absence is the MG's own
/// only while most seats answered in the same round: this FA could reach
/// them, so it could have reached the absent ones. When half or more are
/// absent the fault may be this FA's connection, and no MG is blamed.
pub(crate) fn attributable_absences(
    round: impl IntoIterator<Item = (PublicKey, bool)>,
) -> Vec<PublicKey> {
    let mut seats = 0_usize;
    let mut absent = Vec::new();
    for (mg_id, is_absent) in round {
        seats += 1;
        if is_absent {
            absent.push(mg_id);
        }
    }
    if absent.len() * 2 < seats {
        absent
    } else {
        Vec::new()
    }
}

/// Whether an MG that last failed a ceremony at `failed_at` is still skipped
/// at `now`, both in Unix seconds. A failure dated after `now`, as after the
/// clock was set back, is not trusted to skip anyone.
pub(crate) fn skips(failed_at: u64, now: u64) -> bool {
    failed_at <= now && now - failed_at < CEREMONY_FAILURE_SKIP.as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MgOperation;

    fn mg(byte: u8) -> PublicKey {
        let secret = nostr_sdk::SecretKey::from_slice(&[byte; 32]).unwrap();
        nostr_sdk::Keys::new(secret).public_key()
    }

    fn failure(cause: MgCause) -> FaError {
        FaError::Mg(MgFailure {
            mg: mg(9).to_hex().parse().unwrap(),
            operation: MgOperation::GetStatus,
            cause,
        })
    }

    /// Not answering is absence; answering, even with a refusal, is not.
    #[test]
    fn only_a_seat_that_did_not_answer_is_absent() {
        for cause in [
            MgCause::Timeout,
            MgCause::Transport,
            MgCause::SeatUnavailable,
        ] {
            assert!(is_absence(&failure(cause.clone())), "{cause}");
        }
        for cause in [
            MgCause::InvalidResponse,
            MgCause::NotRunning,
            MgCause::InvalidDkgInput,
            MgCause::Unauthorized,
            MgCause::Other,
        ] {
            assert!(!is_absence(&failure(cause.clone())), "{cause}");
        }
        assert!(!is_absence(&FaError::Timeout("run".to_owned())));
    }

    /// Absent seats are blamed only while fewer than half the seats are
    /// absent; at half or more nobody is, since the fault may be this FA's.
    #[test]
    fn a_minority_of_absent_seats_is_blamed_and_a_half_is_not() {
        let round = |absent: &[bool]| {
            attributable_absences(
                absent
                    .iter()
                    .enumerate()
                    .map(|(index, absent)| (mg(index as u8 + 1), *absent)),
            )
        };
        assert_eq!(round(&[false, false, false, false]), vec![]);
        assert_eq!(round(&[false, true, false, false]), vec![mg(2)]);
        assert_eq!(round(&[true, false, true, false]), vec![]);
        assert_eq!(round(&[true, true, true, true]), vec![]);
        assert_eq!(
            round(&[true, false, false, true, false, false, false]),
            vec![mg(1), mg(4)]
        );
        assert_eq!(
            round(&[true, true, true, false, false, false, false]),
            vec![mg(1), mg(2), mg(3)]
        );
        assert_eq!(
            round(&[true, true, true, true, false, false, false]),
            vec![]
        );
        // One seat alone proves nothing about who failed.
        assert_eq!(round(&[true]), vec![]);
    }

    /// The skip lasts exactly one day from the last failure, and a failure
    /// dated in the future skips nobody.
    #[test]
    fn the_skip_ends_one_day_after_the_failure() {
        let day = 24 * 60 * 60;
        assert!(skips(1_000, 1_000));
        assert!(skips(1_000, 1_000 + day - 1));
        assert!(!skips(1_000, 1_000 + day));
        assert!(!skips(1_001, 1_000));
    }
}
