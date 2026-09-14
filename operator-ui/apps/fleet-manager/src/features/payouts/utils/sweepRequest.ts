import { newIdempotencyKey } from '@operator-ui/common-ui';

/** A sweep request id and the destination the dashboard held when it was first
 *  sent. `undefined` means the destination had not been read at that moment. */
export interface SweepRequest {
  destination: string | null | undefined;
  id: string;
}

/**
 * The request a sweep sends. A failed sweep retries under its id, because a lost
 * response may hide a started payment. The daemon keeps an id's first destination
 * (crates/fman/specs/SPEC-admin-socket.md), so once the stored destination has
 * changed the next sweep takes a new id. A destination not read on either side
 * proves no change, so the id stays.
 */
export const sweepRequestFor = (
  pending: SweepRequest | null,
  destination: string | null | undefined
): SweepRequest => {
  if (pending === null) return { destination, id: newIdempotencyKey() };
  const hasChanged =
    pending.destination !== undefined &&
    destination !== undefined &&
    pending.destination !== destination;
  return hasChanged ? { destination, id: newIdempotencyKey() } : pending;
};
