/** Characters a copy or paste can carry that the operator cannot see: soft
 *  hyphen, zero-width space and joiners, direction marks, word joiner, and byte
 *  order mark. */
const INVISIBLE_CHARACTERS = /[\u00AD\u200B-\u200F\u2060\uFEFF]/g;

/**
 * What the operator meant to type. Compatibility forms fold to plain ones —
 * full-width letters and digits, no-break and thin spaces — invisible
 * characters are dropped, and the ends are trimmed.
 *
 * Not for passwords: the daemon compares a password byte for byte.
 */
export const normalizeFieldText = (input: string): string =>
  input.normalize('NFKC').replace(INVISIBLE_CHARACTERS, '').trim();
