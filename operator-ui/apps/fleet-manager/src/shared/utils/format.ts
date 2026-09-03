// Stand-in for an amount the daemon has not reported yet, or could not report.
// An unknown balance is never rendered as "0 sats" — zero is a fact about the
// wallet, and claiming it when the figure never arrived is a lie the operator
// cannot see through.
export const UNKNOWN_AMOUNT = '—';

// The daemon returns amounts as integer msat (`*_msat`) — always
// convert to sats and label the unit (MVP-SPEC rule 6). `null`/`undefined` means
// "not known", which is why the parameter is nullable rather than defaulted: a
// caller cannot silently coerce a missing figure into a zero.
export const formatSats = (amountMsat: number | bigint | string | null | undefined): string =>
  amountMsat === null || amountMsat === undefined
    ? UNKNOWN_AMOUNT
    : `${(BigInt(amountMsat) / 1000n).toLocaleString('en-US')} sats`;

export const formatDate = (timestampMs: number): string =>
  new Date(timestampMs).toISOString().slice(0, 10);

// The daemon reports relay check times as whole seconds since the epoch
// (admin.rs `checked_at`), not milliseconds. Rendered to the minute, in UTC: a
// "last checked" line is read for recency, and a locale-dependent rendering
// would make its tests assert the runner's timezone instead of the value.
export const formatCheckedAt = (unixSeconds: number): string =>
  `${new Date(unixSeconds * 1000).toISOString().slice(0, 16).replace('T', ' ')} UTC`;
