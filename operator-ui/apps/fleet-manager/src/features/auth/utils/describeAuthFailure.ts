import { InvalidPasswordError } from '@/shared/api/authenticate';
import { HttpStatusError, NetworkError, ProtocolError } from '@/shared/api/errors';

// "Incorrect password" is true of exactly one failure: the 401 POST /api/auth
// answers a wrong password with. Saying it after a dead daemon or a 500 sends
// an operator to rotate a credential that was never rejected, while the real
// fault — the service — goes unmentioned.
//
// What each other line may claim is bounded by what was observed. Only the
// transport failure can say the password went unchecked: nothing was served, so
// nothing read it. A 5xx is an answer — the request reached the service, which
// then failed somewhere inside handling it, possibly after checking the password
// — so that line states the status it observed and stops there.
export const describeAuthFailure = (error: unknown): string => {
  if (error instanceof InvalidPasswordError) {
    return 'Incorrect password. Try again.';
  }
  if (error instanceof NetworkError) {
    return "Can't reach the fleet manager, so the password was never checked. Make sure the service is running, then try again.";
  }
  if (error instanceof HttpStatusError && error.status >= 500) {
    return `The fleet manager failed while signing in (HTTP ${error.status}). That is a fault in the service, not a wrong password. Check the service, then try again.`;
  }
  if (error instanceof HttpStatusError) {
    return `The fleet manager refused the sign-in (HTTP ${error.status}). A wrong password answers 401, so this is a different fault.`;
  }
  if (error instanceof ProtocolError) {
    return 'The sign-in reply could not be read. Check that the fleet manager, and nothing in front of it, is answering, then try again.';
  }
  return 'Sign-in failed. Try again.';
};
