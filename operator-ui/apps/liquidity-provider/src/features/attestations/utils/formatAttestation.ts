import { truncateMiddle } from '@operator-ui/common-ui';
import type { AttestationKind, AttestationSubject } from '@operator-ui/types';

export const ATTESTATION_KIND_LABELS: Record<AttestationKind, string> = {
  holder_authorization: 'Holder authorization',
  issuer_credential: 'Issuer credential',
  issuer_authority: 'Issuer authority'
};

export const formatSubject = (subject: AttestationSubject): string => {
  if ('provider' in subject) return `Provider ${truncateMiddle(subject.provider)}`;
  if ('holder' in subject) return `Holder ${truncateMiddle(subject.holder)}`;
  return `Issuer ${truncateMiddle(subject.issuer)}`;
};
