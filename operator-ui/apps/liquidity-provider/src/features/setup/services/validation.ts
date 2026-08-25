import type { ConfigDraft } from '@/features/setup/services/draft';

type FieldErrors = Record<string, string>;

const isUrl = (value: string): boolean => {
  try {
    new URL(value);
    return true;
  } catch {
    return false;
  }
};

export const validateNetwork = (draft: ConfigDraft): FieldErrors => {
  const validNetworks = ['signet', 'bitcoin', 'testnet', 'regtest'];
  if (!validNetworks.includes(draft.network)) {
    return { network: 'Select a network.' };
  }
  return {};
};

export const validateGateway = (draft: ConfigDraft): FieldErrors => {
  const errors: FieldErrors = {};
  const { gateway_name, admin_url } = draft.gateway;
  if (gateway_name.trim() === '') {
    errors.gateway_name = 'Enter a gateway name.';
  }
  if (!isUrl(admin_url)) {
    errors.admin_url = 'Enter a valid URL.';
  }
  // Read from the gateway, never typed: it is frozen at first setup and decides
  // which gateway an accepted allocation pays. The step offers a control that
  // fetches it; this refuses to move on until it has been fetched.
  if (!draft.gateway.gateway_id) {
    errors.gateway_id = 'Connect to the gateway to read its identity.';
  }
  // The wizard is a first setup, so there is no stored credential for a blank
  // box to mean "keep". On the settings screen, which reuses this step, blank
  // does mean keep and this validator is not applied to it.
  if (draft.secrets.gatewayAdminCredential.trim() === '') {
    errors.admin_credential = 'Enter the admin credential.';
  }
  return errors;
};

export const validateChainObserver = (draft: ConfigDraft): FieldErrors => {
  const { backend } = draft.chain_observer;
  if (!isUrl(backend.url)) {
    return { url: 'Enter a valid URL.' };
  }
  return {};
};

export const validateRelaysEndpoint = (draft: ConfigDraft): FieldErrors => {
  const errors: FieldErrors = {};
  const hasRelay = draft.relays.length > 0;
  const allWss = draft.relays.every((relay) => relay.startsWith('wss://'));
  if (!hasRelay) {
    errors.relays = 'Add at least one relay.';
  } else if (!allWss) {
    errors.relays = 'Every relay must start with wss://.';
  }
  // The advertised address is not asked for: for an Iroh endpoint it is the
  // daemon's node id, derived from the provider identity, and it is empty until
  // the transport binds. Requiring it here would block setup on exactly the
  // deployments that cannot supply it.
  // Not `<= 0`: the field is free text parsed with Number(), and Number('abc')
  // is NaN, which fails every comparison and so passed this check. NaN is then
  // serialised as null and the daemon reads it as a missing interval.
  if (!(draft.advertisement.republish_interval > 0)) {
    errors.republish_interval = 'Republish interval must be greater than zero.';
  }
  return errors;
};

export const validatePolicyCapacity = (draft: ConfigDraft): FieldErrors => {
  const errors: FieldErrors = {};
  const { capacity, replenishment, policy } = draft;
  if (capacity.supported_sources.length < 1) {
    errors.supported_sources = 'Select at least one funding source.';
  }
  if (capacity.mode === 'explicit_cap' && !((capacity.explicit_cap ?? 0) > 0)) {
    errors.explicit_cap = 'Enter a cap greater than zero.';
  }
  if (replenishment.warning_threshold < 0) {
    errors.warning_threshold = 'Threshold must be zero or more.';
  }
  if (replenishment.critical_threshold < 0) {
    errors.critical_threshold = 'Threshold must be zero or more.';
  }
  const attesters = policy.accepted_attester_policies;
  if (
    attesters.length < 1 ||
    attesters.some((policyRow) => policyRow.attester_pubkey.trim() === '')
  ) {
    errors.accepted_attester_policies = 'Add at least one attester with a public key.';
  }
  return errors;
};

const noValidation = (): FieldErrors => ({});

export const STEP_VALIDATORS: ((draft: ConfigDraft) => FieldErrors)[] = [
  validateNetwork,
  validateGateway,
  validateChainObserver,
  validateRelaysEndpoint,
  validatePolicyCapacity,
  noValidation,
  noValidation
];
