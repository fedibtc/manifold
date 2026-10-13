import { describe, expect, it } from 'vitest';
import { toNpub } from '@/shared/utils/npub';

// Cross-implementation vector: this hex/npub pair is the fixture credential-app
// asserts its own holder-profile rendering against, so a match here means an
// operator can compare the two screens character for character.
const CREDENTIAL_APP_HEX = '7e7e9c42a91bfef19fa929e5fda1b72e0ebc1a4c1141673e2794234d86addf4e';
const CREDENTIAL_APP_NPUB = 'npub10elfcs4fr0l0r8af98jlmgdh9c8tcxjvz9qkw038js35mp4dma8qzvjptg';

describe('toNpub', () => {
  it('should encode a hex pubkey the way a holder application renders it', () => {
    expect(toNpub(CREDENTIAL_APP_HEX)).toBe(CREDENTIAL_APP_NPUB);
  });

  it('should encode an observed holder key from a live relay', () => {
    expect(toNpub('5798e0d9c19fe0a1e7b843578f78b570a634d94a1cb5f83da9c0074563b297ae')).toBe(
      'npub127vwpkwpnls2reacgdtc7794wznrfk22rj6ls0dfcqr52cajj7hqwfxl8t'
    );
  });

  it('should accept an uppercase key, since the encoding is of the bytes', () => {
    expect(toNpub(CREDENTIAL_APP_HEX.toUpperCase())).toBe(CREDENTIAL_APP_NPUB);
  });

  it('should refuse a key that is not 32 bytes', () => {
    expect(toNpub('abcd')).toBeUndefined();
  });

  it('should refuse a key that is not hexadecimal', () => {
    expect(toNpub('z'.repeat(64))).toBeUndefined();
  });

  it('should refuse an empty key', () => {
    expect(toNpub('')).toBeUndefined();
  });
});
