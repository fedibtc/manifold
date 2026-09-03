// A caller-generated idempotency identity for a money-moving admin request —
// `request_id` on a sweep, `withdrawal_intent_id` on a withdrawal.
//
// An operator dashboard is commonly reached over plain http at its host address,
// and in that non-secure context `crypto.randomUUID` is not defined at all —
// reading it took the whole Payouts screen down. `getRandomValues` carries no
// such restriction, so the v4 UUID is assembled from it directly rather than
// branching on whichever API this deployment happens to expose: one path, the
// same strength and the same shape on every origin, and no second path that only
// ever runs where it cannot be tested.
export const newIdempotencyKey = (): string => {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  bytes[6] = (bytes[6] & 0x0f) | 0x40; // RFC 4122 version 4
  bytes[8] = (bytes[8] & 0x3f) | 0x80; // RFC 4122 variant 10x

  const hex = Array.from(bytes, (byte) => byte.toString(16).padStart(2, '0')).join('');
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
};
