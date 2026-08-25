import { AdminApiError, DaemonUnavailableError, NetworkError } from '@/shared/api/errors';

// Turns a thrown adminCall error into user-facing banner copy for the funds
// actions. adminCall maps a 5xx / `unavailable` daemon into a NetworkError, so
// that branch is the daemon-down case; an AdminApiError carries the service
// code + message the operator needs to act on (US-FLIP-061 / US-FLIP-063).
export const describeActionError = (error: unknown): string => {
  // Before the NetworkError branch it inherits from: the daemon answered and
  // said it is not serving right now — a live restore, or a start still
  // building its runtime. Calling that unreachable sends an operator looking
  // for a network fault that does not exist.
  if (error instanceof DaemonUnavailableError) {
    return 'The daemon is not serving requests yet — it is starting or restoring. This clears by itself.';
  }
  if (error instanceof NetworkError) {
    return 'The funds daemon is unreachable. Try again once it is back online.';
  }
  if (error instanceof AdminApiError) {
    return `${error.code}: ${error.message}`;
  }
  return error instanceof Error ? error.message : 'Something went wrong. Please try again.';
};
