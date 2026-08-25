// The draft shape and the secrets held beside it live in `shared/`: the settings
// screen edits the same thing through these same steps, and a feature may not
// import a sibling feature.
import { type ConfigDraft, emptyDraftSecrets } from '@/shared/config/draft';

export type { ConfigDraft, DraftSecrets } from '@/shared/config/draft';
export { emptyDraftSecrets } from '@/shared/config/draft';

export const STEP_LABELS = [
  'Network',
  'Gateway',
  'Chain observer',
  'Relays & endpoint',
  'Policy & capacity',
  'Trust',
  'Review'
] as const;

export const initialDraft: ConfigDraft = {
  network: 'signet',
  gateway: {
    gateway_name: '',
    admin_url: '',
    identity_metadata: []
  },
  chain_observer: { backend: { type: 'esplora', url: '' } },
  relays: [],
  capacity: { mode: 'available_funds', supported_sources: [] },
  funding_policy: {
    fee_reserve: 0,
    confirmations: 1,
    stability_pool_min_fee_rate_ppb: 0,
    in_doubt_review_after_secs: 21600
  },
  replenishment: { warning_threshold: 0, critical_threshold: 0 },
  advertised_endpoint: {
    transport: 'iroh',
    address: '',
    discovery_hints: [],
    rpc_protocol_name: 'fedi/flip/public-liquidity/1'
  },
  advertisement: { republish_interval: 3600, ready_advertisement_enabled: true },
  provider_display: null,
  policy: { accepted_attester_policies: [], supported_networks: ['signet'] },
  secrets: emptyDraftSecrets
};
