import { bech32 } from '@scure/base';

const PUBKEY_BYTES = 32;
const HEX_PUBKEY = /^[0-9a-fA-F]{64}$/;

/**
 * Render a hex Nostr public key the way holder applications render it.
 *
 * The daemon reports raw hex, because raw hex is what the `p` tag and the
 * signed statement carry and what it verified. Holder applications show the
 * bech32 `npub` instead, so an operator comparing the two screens is comparing
 * two encodings of one key. This converts ours to theirs.
 *
 * Returns `undefined` rather than a placeholder when the key does not decode,
 * so a caller shows the original value instead of an invented one.
 */
export const toNpub = (hexPubkey: string): string | undefined => {
  if (!HEX_PUBKEY.test(hexPubkey)) return undefined;

  const bytes = new Uint8Array(PUBKEY_BYTES);
  for (let index = 0; index < PUBKEY_BYTES; index += 1) {
    bytes[index] = Number.parseInt(hexPubkey.slice(index * 2, index * 2 + 2), 16);
  }

  return bech32.encode('npub', bech32.toWords(bytes));
};
