// The lower-case base32 alphabet a mint-v2 token is written in.
const BASE32 = 'abcdefghijklmnopqrstuvwxyz234567';

// About the length of a real token holding a few dozen notes and the
// federation invite, so the Payouts screen lays out the size it will meet.
const MOCK_TOKEN_LENGTH = 900;

/** A stand-in for a mint-v2 ecash token, stable per request id. It only looks
 *  like one: nothing can redeem it. */
export const mockEcashToken = (requestId: string): string => {
  let seed = [...requestId].reduce((hash, char) => (hash * 31 + char.charCodeAt(0)) >>> 0, 7);
  let body = '';
  while (body.length < MOCK_TOKEN_LENGTH) {
    seed = (seed * 1_103_515_245 + 12_345) >>> 0;
    body += BASE32[(seed >>> 16) % BASE32.length];
  }
  return `fedimint${body}`;
};
