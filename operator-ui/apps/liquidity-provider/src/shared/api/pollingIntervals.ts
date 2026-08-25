// Central polling policy (02-requirements-baseline.md:108-116). All hooks that
// poll must pull their interval from here instead of hardcoding milliseconds.
export const POLL_ACTIVE_MS = 5_000; // allocations, wallet ops in review
export const POLL_STANDARD_MS = 30_000; // health, funds, advertisement
export const POLL_SETUP_MS = 60_000;

// The §6 table has no row for the attestation list. It changes only when the
// operator installs or removes a credential, so it takes the slowest documented
// cadence rather than inventing a faster one. Give it its own name: sharing
// POLL_SETUP_MS would tie an unrelated screen to whatever setup's cadence
// becomes. Revisit once §6 grows an entry.
export const POLL_SLOW_MS = 60_000;
