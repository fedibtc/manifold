import { truncateMiddle } from '@operator-ui/common-ui';
import { describe, expect, it } from 'vitest';
import {
  ATTESTATION_KIND_LABELS,
  formatSubject
} from '@/features/attestations/utils/formatAttestation';

describe('ATTESTATION_KIND_LABELS', () => {
  it('should map every kind to a human-readable label', () => {
    expect(ATTESTATION_KIND_LABELS.holder_authorization).toBe('Holder authorization');
    expect(ATTESTATION_KIND_LABELS.issuer_credential).toBe('Issuer credential');
    expect(ATTESTATION_KIND_LABELS.issuer_authority).toBe('Issuer authority');
  });
});

describe('formatSubject', () => {
  it('should label the present subject variant with a truncated pubkey', () => {
    const pubkey = '02aabbccddeeff00112233445566778899aabbccddeeff001122334455667788';
    expect(formatSubject({ provider: pubkey })).toBe(`Provider ${truncateMiddle(pubkey)}`);
    expect(formatSubject({ holder: pubkey })).toBe(`Holder ${truncateMiddle(pubkey)}`);
    expect(formatSubject({ issuer: pubkey })).toBe(`Issuer ${truncateMiddle(pubkey)}`);
  });
});
