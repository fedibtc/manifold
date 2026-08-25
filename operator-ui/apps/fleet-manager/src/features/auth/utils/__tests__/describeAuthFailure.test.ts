import { InvalidPasswordError } from '@/shared/api/authenticate';
import { AdminApiError, HttpStatusError, NetworkError, ProtocolError } from '@/shared/api/errors';
import { describeAuthFailure } from '../describeAuthFailure';

it('should blame the password only for a real 401', () => {
  expect(describeAuthFailure(new InvalidPasswordError())).toBe('Incorrect password. Try again.');
});

it('should blame the connection, not the password, when the daemon never answered', () => {
  const message = describeAuthFailure(new NetworkError());

  expect(message).toMatch(/can't reach the fleet manager/i);
  expect(message).not.toMatch(/incorrect password/i);
});

it('should give a server-side failure its own message stating the status', () => {
  expect(describeAuthFailure(new HttpStatusError(500))).toBe(
    'The fleet manager failed while signing in (HTTP 500). That is a fault in the service, not a wrong password. Check the service, then try again.'
  );
});

// A 5xx is an answer: the request reached the service, which may well have read
// the password before failing. Saying it went unchecked states something this side
// cannot observe, and sends the operator looking in the wrong place.
it('should not claim the password went unchecked when the service answered with a server error', () => {
  const message = describeAuthFailure(new HttpStatusError(503));

  expect(message).toMatch(/503/);
  expect(message).not.toMatch(/never checked|not checked|could not be checked/i);
  expect(message).not.toMatch(/can't reach the fleet manager/i);
  expect(message).not.toMatch(/incorrect password/i);
});

it('should state the status when the fleet manager refuses the sign-in for another reason', () => {
  expect(describeAuthFailure(new HttpStatusError(400))).toBe(
    'The fleet manager refused the sign-in (HTTP 400). A wrong password answers 401, so this is a different fault.'
  );
});

it('should not blame the password when the answer was not the auth protocol', () => {
  const message = describeAuthFailure(new ProtocolError());

  expect(message).toMatch(/could not be read/i);
  expect(message).not.toMatch(/incorrect password/i);
});

it('should fall back without blaming the password for an unrecognised failure', () => {
  expect(describeAuthFailure(new AdminApiError('boom'))).toBe('Sign-in failed. Try again.');
});
