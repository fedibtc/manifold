// Attestation fixtures for the FLIP mock server. Seeded into ready scenarios
// so Settings → Attestations and the Trust step have rows to render. Keep
// aligned with @operator-ui/types AttestationPayloadInfo.

import type { AttestationPayloadInfo } from '@operator-ui/types';
// The holder and issuer-credential entries are the real Rust-generated
// contract fixture (see A4 remediation task), so this scenario data can't
// drift from what the daemon's serde impls actually produce. Regenerate via
// `just gen-contract-fixtures`; never hand-edit the JSON.
import attestationsFixture from '@operator-ui/types/fixtures/attestations.json';

const providerPubkey = '02cc'.padEnd(66, '0');
const issuerPubkey = '03bb'.padEnd(66, '0');

export const seededAttestations: AttestationPayloadInfo[] = [
  ...(attestationsFixture.payloads as AttestationPayloadInfo[]),
  {
    id: 'att-issuer-auth-01',
    kind: 'issuer_authority',
    issuer: issuerPubkey,
    subject: { provider: providerPubkey },
    ingested_at: 1784475000, // 2026-07-19T15:30:00Z
    valid: false
  }
];
