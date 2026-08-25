//! Per-window rate bound on outbound trust-verification work.
//!
//! Verifying one `RequestLiquidity` costs invite preview, advertisement
//! resolution, and revocation lookups — all outbound, all against third
//! parties. A rejection that happens *after* those stages is stateless by
//! design: nothing is persisted, so a retry re-evaluates from scratch, which is
//! what lets a request rejected for capacity yesterday be accepted today.
//!
//! Put together, requests that reach a late rejection can be delivered again and
//! again, and each delivery repeats the whole outbound path. The Iroh protocol's
//! 128-handler limit bounds how many run at once, not their rate.
//!
//! This is the per-federation rate bound that closes it.
//!
//! It is keyed by the canonical federation id parsed from the invite and
//! authenticated by the FMan endorsement. The verification pipeline charges it
//! inside the admission gate after local authentication but before its live
//! revocation lookup or any other outbound stage. The requester-declared federation id is deliberately not a key: it is
//! not trusted until the later preview join.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long one federation's allowance covers.
pub(crate) const DEFAULT_VERIFICATION_WINDOW: Duration = Duration::from_secs(300);

/// Verification runs one federation may cost inside a window.
///
/// Set for an honest requester retrying a handful of times — a rejected
/// request is worth retrying after fixing whatever it named — while still
/// being a finite number rather than none.
pub(crate) const DEFAULT_VERIFICATIONS_PER_WINDOW: u32 = 12;

/// Federations tracked at once.
///
/// The map is keyed by locally authenticated invite-derived ids, but an FI can
/// hold endorsements for many federations, so the runtime map needs a ceiling.
pub(crate) const DEFAULT_TRACKED_FEDERATIONS: usize = 4096;

struct Entry {
    window_started: Instant,
    spent: u32,

    /// Whether this window's exhaustion has already been logged.
    ///
    /// The denial repeats for every further request inside the window, and the
    /// caller driving it is exactly the one an operator would be warned about,
    /// so an unconditional line would let that caller size FLIP's log.
    exhaustion_logged: bool,
}

/// Per-federation allowance for outbound verification work.
pub(crate) struct VerificationBudget {
    window: Duration,
    per_window: u32,
    max_tracked: usize,
    entries: Mutex<HashMap<String, Entry>>,
}

impl Default for VerificationBudget {
    fn default() -> Self {
        Self::new(
            DEFAULT_VERIFICATION_WINDOW,
            DEFAULT_VERIFICATIONS_PER_WINDOW,
            DEFAULT_TRACKED_FEDERATIONS,
        )
    }
}

impl VerificationBudget {
    pub(crate) fn new(window: Duration, per_window: u32, max_tracked: usize) -> Self {
        Self {
            window,
            per_window,
            max_tracked,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Charges one verification run to `federation_id`.
    ///
    /// Returns false when this federation has spent its allowance for the
    /// current window, in which case the caller must not perform the outbound
    /// work.
    ///
    /// `now` is a parameter so the window can be exercised without sleeping.
    pub(crate) fn try_spend(&self, federation_id: &str, now: Instant) -> bool {
        let mut entries = self.entries.lock().expect("verification budget lock");

        if let Some(entry) = entries.get_mut(federation_id) {
            if now.duration_since(entry.window_started) >= self.window {
                entry.window_started = now;
                entry.spent = 1;
                entry.exhaustion_logged = false;
                return true;
            }
            if entry.spent >= self.per_window {
                if !entry.exhaustion_logged {
                    entry.exhaustion_logged = true;
                    tracing::warn!(
                        federation_id,
                        per_window = self.per_window,
                        window_secs = self.window.as_secs(),
                        "federation spent its verification allowance for this window; \
                         further requests for it are refused until the window renews"
                    );
                }
                return false;
            }
            entry.spent += 1;
            return true;
        }

        if entries.len() >= self.max_tracked {
            entries.retain(|_, entry| now.duration_since(entry.window_started) < self.window);
        }
        if entries.len() >= self.max_tracked {
            // Every tracked federation is still inside its window. Refuse an
            // unseen federation rather than evicting a live entry: eviction
            // would let authenticated multi-federation pressure reset a spent
            // federation and exceed its per-window allowance.
            //
            // Warned every time, unlike the per-federation case above: this one
            // refuses a federation that has spent nothing, so it is FLIP
            // turning away work it would otherwise do, and the caller cannot
            // choose to be the one refused.
            tracing::warn!(
                tracked = entries.len(),
                max_tracked = self.max_tracked,
                "verification budget is tracking its ceiling of federations; \
                 refusing an untracked one until a window renews"
            );
            return false;
        }
        entries.insert(
            federation_id.to_owned(),
            Entry {
                window_started: now,
                spent: 1,
                exhaustion_logged: false,
            },
        );
        true
    }
}

#[cfg(test)]
#[path = "../tests/verification_budget.rs"]
mod tests;
