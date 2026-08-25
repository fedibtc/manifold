import type { WalletDrainStatus } from '@operator-ui/types';

export const walletStatus = (availableEcashMsat: number | null): WalletDrainStatus => ({
  available_ecash_msat: availableEcashMsat,
  economically_sweepable_recipient_msat: availableEcashMsat,
  encumbered_outgoing_msat: availableEcashMsat === null ? null : 0,
  outgoing: availableEcashMsat === null ? null : [],
  active_operation_count: 0,
  query_errors: availableEcashMsat === null ? ['available_ecash'] : [],
  drain_state:
    availableEcashMsat === null ? 'unknown' : availableEcashMsat === 0 ? 'drained' : 'sweepable'
});
