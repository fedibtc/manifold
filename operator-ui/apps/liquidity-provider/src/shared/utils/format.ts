import type { Sats, Timestamp } from '@operator-ui/types';

// Stand-in for an amount the daemon has not reported yet, or could not report.
// An unknown balance is never rendered as "0 sats" — zero is a fact about the
// wallet, and claiming it when the figure never arrived is a lie the operator
// cannot see through.
export const UNKNOWN_AMOUNT = '—';

// Grouped digits with en-US thousands separators, no unit. The shared core every
// amount formatter builds on, so grouping stays identical across features.
export const formatAmount = (amount: Sats | null | undefined): string =>
  amount === null || amount === undefined ? UNKNOWN_AMOUNT : amount.toLocaleString('en-US');

// Grouped amount with the lowercase sats unit (funds / overview wireframes).
// Nullable by design: a caller cannot silently coerce a missing figure to zero.
export const formatSats = (amount: Sats | null | undefined): string =>
  amount === null || amount === undefined ? UNKNOWN_AMOUNT : `${formatAmount(amount)} sats`;

// Turn a snake_case wire token into a human label ("gateway_funding" → "gateway funding").
export const humanizeToken = (token: string): string => token.replace(/_/g, ' ');

// The wire Timestamp is Unix seconds (Rust: serde(transparent) u64) — the codec
// every timestamp consumer goes through to reach a JS Date.
export const timestampToDate = (ts: Timestamp): Date => new Date(ts * 1000);

// Date portion of a Unix-seconds timestamp — deterministic, timezone-agnostic.
export const formatDate = (timestamp: Timestamp): string =>
  timestampToDate(timestamp).toISOString().slice(0, 10);

// Date and minute of a Unix-seconds timestamp, in UTC. Same deterministic,
// timezone-agnostic treatment as formatDate: an operator comparing the
// dashboard against daemon logs should read one clock, not two.
export const formatDateTime = (timestamp: Timestamp): string =>
  timestampToDate(timestamp).toISOString().slice(0, 16).replace('T', ' ');
