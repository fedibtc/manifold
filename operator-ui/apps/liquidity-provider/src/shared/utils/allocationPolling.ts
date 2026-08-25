import type { ItemAllocationStatus } from '@operator-ui/types';

// Shared by the allocations list and detail hooks to decide whether the
// active polling cadence still applies (public.rs `ItemAllocationStatus`).
const TERMINAL_STATUSES: ReadonlySet<ItemAllocationStatus> = new Set([
  'completed',
  'failed',
  'cancelled'
]);

export const hasNonTerminalStatus = (statuses: ItemAllocationStatus[]): boolean =>
  statuses.some((status) => !TERMINAL_STATUSES.has(status));
