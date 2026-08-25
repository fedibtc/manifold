import type { SetupConfigView } from '@operator-ui/types';
import { type ConfigDraft, emptyDraftSecrets } from '@/shared/config/draft';

// Adapts the read-shape SetupConfigView (from get_provider_config) into an
// editable draft. Every config field maps 1:1, because the view no longer
// withholds any of them: the two secrets it used to omit are not config fields
// at all now — the daemon stores them by name, a config write cannot touch
// them, and they are seeded blank here meaning "unchanged".
export const seedDraftFromView = (view: SetupConfigView): ConfigDraft => {
  const { backend } = view.chain_observer;
  const chainObserverBackend =
    backend.type === 'esplora'
      ? { type: 'esplora' as const, url: backend.url }
      : {
          type: 'bitcoind' as const,
          url: backend.url,
          username: backend.username ?? null
        };

  return {
    network: view.network,
    gateway: {
      gateway_id: view.gateway.gateway_id,
      gateway_name: view.gateway.gateway_name,
      admin_url: view.gateway.admin_url,
      identity_metadata: view.gateway.identity_metadata
    },
    chain_observer: { backend: chainObserverBackend },
    relays: view.relays,
    capacity: view.capacity,
    funding_policy: view.funding_policy,
    replenishment: view.replenishment,
    advertised_endpoint: view.advertised_endpoint,
    advertisement: view.advertisement,
    provider_display: view.provider_display ?? null,
    policy: view.policy,
    secrets: { ...emptyDraftSecrets }
  };
};
