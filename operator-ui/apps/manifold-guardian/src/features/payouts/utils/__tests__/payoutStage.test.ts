import type {
  EcashPayoutState,
  PayoutJobStatusResponse,
  WalletOutgoingOperation
} from '@operator-ui/types';
import { describe, expect, it } from 'vitest';
import { payoutStage } from '../payoutStage';

const scope = { kind: 'payment_federation', federation_id: 'fed1aaa' } as const;
const operation = {
  operation_id: 'op',
  amount_msat: 1_000,
  capped: null,
  committed_at_ms: 1,
  confirmed_at_ms: null
};

const lightning = (state: WalletOutgoingOperation['state'] | null): PayoutJobStatusResponse => ({
  job: {
    request_id: 'r',
    scope,
    method: 'lightning',
    destination: 'a@b.c',
    operation,
    created_at_ms: 1
  },
  payout:
    state === null
      ? null
      : {
          operation_id: 'op',
          rail: 'lnv2',
          state,
          recipient_amount_msat: 1_000,
          contract_amount_msat: 1_010,
          encumbered_msat: 0,
          has_active_state_machines: false
        }
});

const ecash = (
  state: EcashPayoutState | null,
  confirmedAtMs: number | null = null
): PayoutJobStatusResponse => ({
  job: {
    request_id: 'r',
    scope,
    method: 'ecash',
    operation: { ...operation, confirmed_at_ms: confirmedAtMs },
    created_at_ms: 1
  },
  ecash: state
});

describe('payoutStage', () => {
  it.each([
    [lightning(null), false, 'Did not start'],
    [lightning('pending'), true, 'Paying'],
    [lightning('unknown'), true, "Can't check right now"],
    [lightning('succeeded'), false, 'Sent'],
    [lightning('failed_or_refunded'), false, 'Failed, refunded'],
    [ecash(null), false, 'Did not start'],
    [ecash({ state: 'issued', token: null }), true, 'Waiting to be redeemed'],
    [ecash({ state: 'reclaiming' }), true, 'Returning to balance'],
    // The operator's mark closes an issued payout.
    [ecash({ state: 'issued', token: null }, 2), false, 'Redeemed'],
    // A cancel after the mark: the wallet's answer wins over the mark.
    [ecash({ state: 'reclaimed' }, 2), false, 'Returned to balance'],
    [ecash({ state: 'reclaiming' }, 2), true, 'Returning to balance'],
    // A cancel that lost the race found the token redeemed.
    [ecash({ state: 'already_spent', token: null }), false, 'Redeemed'],
    [ecash({ state: 'reclaimed' }), false, 'Returned to balance']
  ])('should place %j', (payout, open, label) => {
    expect(payoutStage(payout)).toMatchObject({ open, label });
  });
});
