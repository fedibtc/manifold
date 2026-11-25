import { newIdempotencyKey } from '@operator-ui/common-ui';
import type { PayoutJob } from '@operator-ui/types';

export type PayoutMethodName = PayoutJob['method'];

/** A sweep request id, with the method and the destination the dashboard held
 *  when it was first sent. `undefined` means the destination had not been read
 *  at that moment. */
export interface SweepRequest {
  method: PayoutMethodName;
  destination: string | null | undefined;
  id: string;
}

/**
 * The request a sweep sends. A failed sweep retries under its id, because a lost
 * response may hide a started payout. The daemon keeps an id's first method and
 * destination (crates/mg/specs/SPEC-admin-socket.md), so once the operator picks
 * the other method, or the stored destination of a Lightning sweep has changed,
 * the next sweep takes a new id. A destination not read on either side proves
 * no change, so the id stays.
 */
export const sweepRequestFor = (
  pending: SweepRequest | null,
  method: PayoutMethodName,
  destination: string | null | undefined
): SweepRequest => {
  const fresh = { method, destination, id: newIdempotencyKey() };
  if (pending === null || pending.method !== method) return fresh;
  const hasChanged =
    method === 'lightning' &&
    pending.destination !== undefined &&
    destination !== undefined &&
    pending.destination !== destination;
  return hasChanged ? fresh : pending;
};
