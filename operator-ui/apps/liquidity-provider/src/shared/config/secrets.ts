import type {
  SetConfigSecretRequest,
  SetConfigSecretResponse,
  SetupConfig
} from '@operator-ui/types';
import { adminCall } from '@/shared/api/adminCall';
import type { ConfigDraft } from '@/shared/config/draft';

// The config the daemon is sent. Secrets are held beside the draft while the
// operator types them and are written by name; a config write cannot carry one,
// which is what stops a blank field from meaning anything at all.
export const toSetupConfig = ({ secrets: _secrets, ...config }: ConfigDraft): SetupConfig => config;

// Writes only the secrets the operator actually typed.
//
// A blank box is "unchanged" and sends nothing. Removing a secret is a separate
// action the operator takes on purpose — never an empty string, which the
// daemon refuses, and never an omission, which it used to read as delete.
//
// Called before the config write, because the daemon validates a candidate
// config against the stored secrets: a credential just typed has to be stored
// before it can be tested.
export const storeDraftSecrets = async (draft: ConfigDraft): Promise<void> => {
  const updates: SetConfigSecretRequest[] = [];
  if (draft.secrets.gatewayAdminCredential.trim() !== '') {
    updates.push({
      secret: 'gateway_admin_credential',
      update: { action: 'set', value: draft.secrets.gatewayAdminCredential }
    });
  }
  if (draft.secrets.chainObserverPassword.trim() !== '') {
    updates.push({
      secret: 'chain_observer_password',
      update: { action: 'set', value: draft.secrets.chainObserverPassword }
    });
  }
  for (const request of updates) {
    await adminCall<SetConfigSecretRequest, SetConfigSecretResponse>('set_config_secret', request);
  }
};

// Removes a stored secret. The explicit half of the pair: the daemon refuses an
// empty value precisely so that this is the only way to take one away.
export const clearDraftSecret = (secret: SetConfigSecretRequest['secret']) =>
  adminCall<SetConfigSecretRequest, SetConfigSecretResponse>('set_config_secret', {
    secret,
    update: { action: 'clear' }
  });
