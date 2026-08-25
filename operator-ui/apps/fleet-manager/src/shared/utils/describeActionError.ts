import {
  AccessDeniedError,
  AdminApiError,
  AuthError,
  HttpStatusError,
  isDaemonUnreachable
} from '@/shared/api/errors';

// FM's admin API has no error-code taxonomy (see AdminApiError) — unlike FLIP's
// describeActionError, there is no `${code}: ${message}` branch to build. The
// transport taxonomy is read apart though: only a failure that means the daemon
// is not serving may offer "try again once it's back".
export const describeActionError = (error: unknown): string => {
  if (error instanceof AuthError) {
    return 'Your session expired. Sign in again.';
  }
  if (error instanceof AccessDeniedError) {
    return 'The fleet manager refused this request. Your session is valid; this account is not allowed to make it.';
  }
  if (error instanceof AdminApiError) {
    return error.message;
  }
  if (isDaemonUnreachable(error)) {
    return "Can't reach the fleet manager. Try again once it's back online.";
  }
  if (error instanceof HttpStatusError) {
    return `The fleet manager refused the request (HTTP ${error.status}).`;
  }
  if (error instanceof Error) {
    return error.message;
  }
  return 'Something went wrong. Please try again.';
};
