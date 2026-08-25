import { describe, expect, it } from 'vitest';
import { type ConfigDraft, initialDraft } from '@/features/setup/services/draft';
import {
  validateChainObserver,
  validateGateway,
  validateNetwork,
  validateRelaysEndpoint
} from '@/features/setup/services/validation';

// The credential is not a gateway config field: it is stored by name, and the
// wizard holds what the operator typed beside the draft.
// gateway_id is read from the gateway, not typed, so a complete draft has one.
const withGateway = (
  patch: Partial<ConfigDraft['gateway']>,
  gatewayAdminCredential = 'secret'
): ConfigDraft => ({
  ...initialDraft,
  gateway: { ...initialDraft.gateway, gateway_id: 'gw-probed', ...patch },
  secrets: { ...initialDraft.secrets, gatewayAdminCredential }
});

describe('validateNetwork', () => {
  it('should accept the default network', () => {
    expect(validateNetwork(initialDraft)).toEqual({});
  });

  it('should reject an unknown network', () => {
    const draft = { ...initialDraft, network: 'litecoin' as ConfigDraft['network'] };
    expect(validateNetwork(draft).network).toBeTruthy();
  });
});

describe('validateGateway', () => {
  it('should flag empty name, non-URL admin_url and empty credential', () => {
    const errors = validateGateway(initialDraft);
    expect(errors.gateway_name).toBeTruthy();
    expect(errors.admin_url).toBeTruthy();
    expect(errors.admin_credential).toBeTruthy();
  });

  it('should pass when all fields are valid', () => {
    const draft = withGateway({
      gateway_name: 'gw',
      admin_url: 'https://gw.example.com'
    });
    expect(validateGateway(draft)).toEqual({});
  });

  it('should flag an unparseable admin_url', () => {
    const draft = withGateway({
      gateway_name: 'gw',
      admin_url: 'not a url'
    });
    expect(validateGateway(draft).admin_url).toBeTruthy();
  });
});

describe('validateChainObserver', () => {
  it('should flag an empty esplora url', () => {
    expect(validateChainObserver(initialDraft).url).toBeTruthy();
  });

  it('should pass a valid esplora url', () => {
    const draft: ConfigDraft = {
      ...initialDraft,
      chain_observer: { backend: { type: 'esplora', url: 'https://esplora.example.com' } }
    };
    expect(validateChainObserver(draft)).toEqual({});
  });

  it('should pass a bitcoind backend with a valid url regardless of credentials', () => {
    const draft: ConfigDraft = {
      ...initialDraft,
      chain_observer: { backend: { type: 'bitcoind', url: 'http://127.0.0.1:8332' } }
    };
    expect(validateChainObserver(draft)).toEqual({});
  });
});

describe('validateRelaysEndpoint', () => {
  it('should flag missing relays', () => {
    const errors = validateRelaysEndpoint(initialDraft);
    expect(errors.relays).toBeTruthy();
  });

  it('should not ask the operator for an advertised address', () => {
    const errors = validateRelaysEndpoint(initialDraft);
    expect(errors.address).toBeUndefined();
  });

  it('should flag a non-wss relay entry', () => {
    const draft: ConfigDraft = {
      ...initialDraft,
      relays: ['http://relay.example.com'],
      advertised_endpoint: { ...initialDraft.advertised_endpoint, address: 'endpoint' }
    };
    expect(validateRelaysEndpoint(draft).relays).toBeTruthy();
  });

  it('should flag a non-positive republish interval', () => {
    const draft: ConfigDraft = {
      ...initialDraft,
      relays: ['wss://relay.example.com'],
      advertised_endpoint: { ...initialDraft.advertised_endpoint, address: 'endpoint' },
      advertisement: { ...initialDraft.advertisement, republish_interval: 0 }
    };
    expect(validateRelaysEndpoint(draft).republish_interval).toBeTruthy();
  });

  it('should pass with a wss relay, an address and a positive interval', () => {
    const draft: ConfigDraft = {
      ...initialDraft,
      relays: ['wss://relay.example.com'],
      advertised_endpoint: { ...initialDraft.advertised_endpoint, address: 'endpoint' }
    };
    expect(validateRelaysEndpoint(draft)).toEqual({});
  });
});
