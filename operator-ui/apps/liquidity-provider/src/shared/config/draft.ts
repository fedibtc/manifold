import type { SetupConfig } from '@operator-ui/types';

// Secrets an operator types while filling a form, held apart from the config
// they belong to.
//
// They are not config fields: the daemon stores them by name and a config write
// cannot touch them. Keeping them apart is what makes a blank field
// unambiguous — a blank the operator never touched is "unchanged", and removing
// a secret is a separate action they take on purpose.
export interface DraftSecrets {
  // Required for a first setup; blank afterwards means "keep the stored one".
  gatewayAdminCredential: string;
  // '' means unchanged. Removing it is the Remove control, not an empty box.
  chainObserverPassword: string;
}

export const emptyDraftSecrets: DraftSecrets = {
  gatewayAdminCredential: '',
  chainObserverPassword: ''
};

// An editable configuration plus the secrets being entered alongside it.
//
// Shared rather than owned by the setup wizard, because the settings screen
// edits the same thing: it mounts the wizard's own steps over a config seeded
// from the daemon. That makes this a shared shape, not a wizard one.
export type ConfigDraft = SetupConfig & { secrets: DraftSecrets };
